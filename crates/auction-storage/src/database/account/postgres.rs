//! PostgreSQL Public account 读取与代理历史查询。
//!
//! 该模块只负责 PostgreSQL 快照查询，结果映射和公共不变量校验由 mapping 模块集中维护。

use super::*;

#[cfg(feature = "postgres")]
pub(super) async fn list_postgres(
    store: &super::PostgresStore,
    user_id: Uuid,
    request: PublicMyBidsRequest,
) -> Result<PublicMyBidsPage, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("开启 PostgreSQL 账户读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置 PostgreSQL 账户读取隔离级别失败"))?;

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
         ORDER BY p.updated_at DESC, p.lot_id
         LIMIT $2 OFFSET $3",
    )
    .bind(user_id)
    .bind(request.proxies.limit)
    .bind(request.proxies.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 用户代理历史失败"))?;

    let mut proxies = Vec::with_capacity(proxy_rows.len());
    for row in proxy_rows {
        let base = map_postgres_proxy_base(row)?;
        let transitions =
            load_postgres_transitions(&mut transaction, base.authorization_id).await?;
        let bid = load_postgres_proxy_bid(&mut transaction, &base).await?;
        proxies.push(build_proxy(base, transitions, bid)?);
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
    validate_page(&page, request)
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
        .begin()
        .await
        .map_err(|_| storage_error("开启 PostgreSQL 用户代理读取事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置 PostgreSQL 用户代理读取隔离级别失败"))?;
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
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
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
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
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
    let transitions = load_postgres_transitions(&mut transaction, base.authorization_id).await?;
    let bid = load_postgres_proxy_bid(&mut transaction, &base).await?;
    let proxy = build_proxy(base, transitions, bid)?;
    transaction.commit().await.map_err(|_| storage_error("提交 PostgreSQL 用户代理事务失败"))?;
    Ok(Some(proxy))
}

#[cfg(feature = "postgres")]
async fn load_postgres_transitions(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    authorization_id: Uuid,
) -> Result<Vec<PublicProxyTransition>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT
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
         WHERE authorization_id = $1
         ORDER BY transition_number",
    )
    .bind(authorization_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 代理转换历史失败"))?;
    rows.into_iter().map(map_postgres_transition).collect()
}

#[cfg(feature = "postgres")]
async fn load_postgres_proxy_bid(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    base: &ProxyBase,
) -> Result<Option<PublicProxyBid>, ApplicationError> {
    let row = sqlx::query(
        "WITH next_authorization AS (
            SELECT min(next_authorization.created_at) AS created_at
            FROM public.bid_proxy_authorizations AS next_authorization
            WHERE next_authorization.lot_id = $1
              AND next_authorization.user_id = $2
              AND next_authorization.usage_number = $4 + 1
         )
         SELECT
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
         FROM public.bids AS b
         JOIN public.lots AS l ON l.id = b.lot_id
         CROSS JOIN next_authorization
         WHERE b.lot_id = $1
           AND b.bidder_user_id = $2
           AND b.mechanism = 'proxy'
           AND b.created_at >= $3
           AND (next_authorization.created_at IS NULL OR b.created_at < next_authorization.created_at)
         ORDER BY b.accepted_state_version DESC, b.fact_index DESC, b.id DESC
         LIMIT 1",
    )
    .bind(base.lot_id)
    .bind(base.user_id)
    .bind(base.created_at)
    .bind(base.usage_number)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 代理最近竞价失败"))?;
    row.map(map_postgres_proxy_bid).transpose()
}
