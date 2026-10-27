//! PostgreSQL Public account 读取与代理历史查询；结果映射和公共不变量校验由 mapping 维护。

use std::collections::HashMap;

use futures_util::TryStreamExt;

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn list_postgres(
    store: &super::PostgresStore,
    user_id: Uuid,
    request: PublicMyBidsRequest,
) -> Result<PublicMyBidsPage, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|_| storage_error("开启 PostgreSQL 账户读取事务失败"))?;

    let bid_rows = sqlx::query(
        "SELECT
            b.id,
            l.auction_id,
            b.lot_id,
            l.category_number AS lot_number,
            b.amount,
            b.accepted_state_version,
            b.fact_index,
            b.mechanism,
            b.created_at,
            COALESCE(l.leader_user_id = b.bidder_user_id AND l.current_price = b.amount, false)::boolean AS leading,
            e.name AS equipment_name,
            e.category AS equipment_category
         FROM public.bids AS b
         JOIN public.lots AS l ON l.id = b.lot_id
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         JOIN public.equipment AS e ON e.id = l.equipment_id
         WHERE b.bidder_user_id = $1
           AND a.status IN ('live', 'final', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
         ORDER BY b.created_at DESC, b.id DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(user_id)
    .bind(request.bids.limit)
    .bind(request.bids.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 用户竞价失败"))?;
    let bids = bid_rows.into_iter().map(map_postgres_bid).collect::<Result<Vec<_>, _>>()?;

    let bid_total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)
         FROM public.bids AS b
         JOIN public.lots AS l ON l.id = b.lot_id
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         WHERE b.bidder_user_id = $1
           AND a.status IN ('live', 'final', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')",
    )
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 PostgreSQL 用户竞价失败"))?;

    let proxy_rows = sqlx::query(
        "SELECT
            p.authorization_id,
            l.auction_id,
            p.lot_id,
            p.user_id,
            l.category_number AS lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM public.bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            )::bigint AS quota_used,
            COALESCE((a.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0) AS quota_limit,
            (a.policy_snapshot ->> 'bid_increment_ppm')::bigint AS policy_bid_increment_ppm,
            (a.policy_snapshot ->> 'minimum_increment')::bigint AS policy_minimum_increment,
            p.mode,
            p.maximum_amount,
            p.status AS proxy_status,
            p.terminal_reason,
            p.state_version,
            p.created_at,
            p.updated_at,
            e.name AS equipment_name,
            e.category AS equipment_category,
            l.start_price,
            l.current_price,
            l.material,
            l.leader_user_id,
            l.slow_authorization_id
         FROM public.bid_proxy_authorizations AS p
         JOIN public.lots AS l ON l.id = p.lot_id
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         JOIN public.equipment AS e ON e.id = l.equipment_id
         WHERE p.user_id = $1
           AND a.status IN ('live', 'final', 'settling', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
         ORDER BY p.updated_at DESC, p.lot_id, p.authorization_id
         LIMIT $2 OFFSET $3",
    )
    .bind(user_id)
    .bind(request.proxies.limit)
    .bind(request.proxies.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 用户代理历史失败"))?;

    let bases =
        proxy_rows.into_iter().map(map_postgres_proxy_base).collect::<Result<Vec<_>, _>>()?;
    let authorization_ids = bases.iter().map(|base| base.authorization_id).collect::<Vec<_>>();
    // 仅补齐当前页授权，避免每条授权分别往返读取转换历史与最近竞价。
    let mut transitions = load_postgres_transitions(&mut transaction, &authorization_ids).await?;
    let mut proxy_bids = load_postgres_proxy_bids(&mut transaction, &authorization_ids).await?;
    let mut proxies = Vec::with_capacity(bases.len());
    for base in bases {
        let history = transitions.remove(&base.authorization_id).unwrap_or_default();
        let bid = proxy_bids.remove(&base.authorization_id);
        proxies.push(build_proxy(base, history, bid)?);
    }

    let proxy_total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)
         FROM public.bid_proxy_authorizations AS p
         JOIN public.lots AS l ON l.id = p.lot_id
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         WHERE p.user_id = $1
           AND a.status IN ('live', 'final', 'settling', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')",
    )
    .bind(user_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 PostgreSQL 用户代理历史失败"))?;

    transaction.commit().await.map_err(|_| storage_error("提交 PostgreSQL 账户读取事务失败"))?;
    let page = PublicMyBidsPage { bids, proxies, bid_total, proxy_total };
    validate_page(page, request)
}

#[cfg(feature = "postgres")]
pub(super) async fn proxy_postgres(
    store: &super::PostgresStore,
    lot_id: Uuid,
    user_id: Uuid,
    authorization_id: Option<Uuid>,
) -> Result<Option<PublicOwnedProxy>, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|_| storage_error("开启 PostgreSQL 用户代理读取事务失败"))?;
    let row = {
        let statement = if authorization_id.is_some() {
            "SELECT
            p.authorization_id,
            l.auction_id,
            p.lot_id,
            p.user_id,
            l.category_number AS lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM public.bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            )::bigint AS quota_used,
            COALESCE((a.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0) AS quota_limit,
            (a.policy_snapshot ->> 'bid_increment_ppm')::bigint AS policy_bid_increment_ppm,
            (a.policy_snapshot ->> 'minimum_increment')::bigint AS policy_minimum_increment,
            p.mode,
            p.maximum_amount,
            p.status AS proxy_status,
            p.terminal_reason,
            p.state_version,
            p.created_at,
            p.updated_at,
            e.name AS equipment_name,
            e.category AS equipment_category,
            l.start_price,
            l.current_price,
            l.material,
            l.leader_user_id,
            l.slow_authorization_id
         FROM public.bid_proxy_authorizations AS p
         JOIN public.lots AS l ON l.id = p.lot_id
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         JOIN public.equipment AS e ON e.id = l.equipment_id
         WHERE p.lot_id = $1 AND p.user_id = $2 AND p.authorization_id = $3
           AND a.status IN ('live', 'final', 'settling', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
         LIMIT 1"
        } else {
            "SELECT
            p.authorization_id,
            l.auction_id,
            p.lot_id,
            p.user_id,
            l.category_number AS lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM public.bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            )::bigint AS quota_used,
            COALESCE((a.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0) AS quota_limit,
            (a.policy_snapshot ->> 'bid_increment_ppm')::bigint AS policy_bid_increment_ppm,
            (a.policy_snapshot ->> 'minimum_increment')::bigint AS policy_minimum_increment,
            p.mode,
            p.maximum_amount,
            p.status AS proxy_status,
            p.terminal_reason,
            p.state_version,
            p.created_at,
            p.updated_at,
            e.name AS equipment_name,
            e.category AS equipment_category,
            l.start_price,
            l.current_price,
            l.material,
            l.leader_user_id,
            l.slow_authorization_id
         FROM public.bid_proxy_authorizations AS p
         JOIN public.lots AS l ON l.id = p.lot_id
         JOIN public.auctions AS a ON a.id = l.auction_id
         JOIN public.worlds AS w ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
         JOIN public.equipment AS e ON e.id = l.equipment_id
         WHERE p.lot_id = $1 AND p.user_id = $2
           AND a.status IN ('live', 'final', 'settling', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
         ORDER BY p.usage_number DESC
         LIMIT 1"
        };
        let mut query = sqlx::query(statement).bind(lot_id).bind(user_id);
        if let Some(authorization_id) = authorization_id {
            query = query.bind(authorization_id);
        }
        query
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| storage_error("读取 PostgreSQL 用户代理失败"))?
    };
    let Some(row) = row else {
        transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 PostgreSQL 用户代理事务失败"))?;
        return Ok(None);
    };
    let base = map_postgres_proxy_base(row)?;
    let authorization_ids = [base.authorization_id];
    let transitions = load_postgres_transitions(&mut transaction, &authorization_ids)
        .await?
        .remove(&base.authorization_id)
        .unwrap_or_default();
    let bid = load_postgres_proxy_bids(&mut transaction, &authorization_ids)
        .await?
        .remove(&base.authorization_id);
    let proxy = build_proxy(base, transitions, bid)?;
    transaction.commit().await.map_err(|_| storage_error("提交 PostgreSQL 用户代理事务失败"))?;
    Ok(Some(proxy))
}

#[cfg(feature = "postgres")]
async fn load_postgres_transitions(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authorization_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<PublicProxyTransition>>, ApplicationError> {
    let mut histories = HashMap::<Uuid, Vec<PublicProxyTransition>>::new();
    if authorization_ids.is_empty() {
        return Ok(histories);
    }
    let mut rows = sqlx::query(
        "SELECT
            authorization_id,
            transition_number,
            transition_kind AS kind,
            old_maximum_amount,
            new_maximum_amount,
            old_mode,
            new_mode,
            old_status,
            new_status,
            reason,
            resulting_state_version,
            created_at
         FROM public.bid_proxy_authorization_transitions
         WHERE authorization_id = ANY($1)
         ORDER BY authorization_id, transition_number",
    )
    .bind(authorization_ids)
    .fetch(&mut **transaction);
    while let Some(row) =
        rows.try_next().await.map_err(|_| storage_error("读取 PostgreSQL 代理转换历史失败"))?
    {
        let authorization_id =
            get_pg(&row, "authorization_id", "读取 PostgreSQL 代理授权标识失败")?;
        histories.entry(authorization_id).or_default().push(map_postgres_transition(row)?);
    }
    Ok(histories)
}

#[cfg(feature = "postgres")]
async fn load_postgres_proxy_bids(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authorization_ids: &[Uuid],
) -> Result<HashMap<Uuid, PublicProxyBid>, ApplicationError> {
    let mut bids = HashMap::new();
    if authorization_ids.is_empty() {
        return Ok(bids);
    }
    // 每个授权仍按下一次 usage 的创建时间截断；分页及完整历史读取使用同一归属规则。
    let mut rows = sqlx::query(
        "SELECT
            p.authorization_id,
            b.id,
            l.auction_id,
            b.lot_id,
            l.category_number AS lot_number,
            b.amount,
            b.accepted_state_version,
            b.fact_index,
            b.mechanism,
            b.created_at,
            COALESCE(l.leader_user_id = b.bidder_user_id AND l.current_price = b.amount, false)::boolean AS leading
         FROM public.bid_proxy_authorizations AS p
         JOIN public.lots AS l ON l.id = p.lot_id
         CROSS JOIN LATERAL (
            SELECT min(next_authorization.created_at) AS created_at
            FROM public.bid_proxy_authorizations AS next_authorization
            WHERE next_authorization.lot_id = p.lot_id
              AND next_authorization.user_id = p.user_id
              AND next_authorization.usage_number = p.usage_number + 1
         ) AS next_authorization
         CROSS JOIN LATERAL (
            SELECT b.id, b.lot_id, b.bidder_user_id, b.amount, b.accepted_state_version,
                   b.fact_index, b.mechanism, b.created_at
            FROM public.bids AS b
            WHERE b.lot_id = p.lot_id
              AND b.bidder_user_id = p.user_id
              AND b.mechanism = 'proxy'
              AND b.created_at >= p.created_at
              AND (next_authorization.created_at IS NULL OR b.created_at < next_authorization.created_at)
            ORDER BY b.accepted_state_version DESC, b.fact_index DESC, b.id DESC
            LIMIT 1
         ) AS b
         WHERE p.authorization_id = ANY($1)",
    )
    .bind(authorization_ids)
    .fetch(&mut **transaction);
    while let Some(row) =
        rows.try_next().await.map_err(|_| storage_error("读取 PostgreSQL 代理最近竞价失败"))?
    {
        let authorization_id =
            get_pg(&row, "authorization_id", "读取 PostgreSQL 代理授权标识失败")?;
        bids.insert(authorization_id, map_postgres_proxy_bid(row)?);
    }
    Ok(bids)
}
