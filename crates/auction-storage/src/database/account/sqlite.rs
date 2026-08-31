//! SQLite Lite Public account 读取与代理历史查询。
//!
//! Lite 查询只使用已声明的本地能力，不把 PostgreSQL 的完整锁与字段假设扩散到公开端。

use super::*;

#[cfg(feature = "sqlite")]
pub(super) async fn list_sqlite(
    store: &super::SqliteStore,
    user_id: Uuid,
    request: PublicMyBidsRequest,
) -> Result<PublicMyBidsPage, ApplicationError> {
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启 SQLite 账户读取事务失败"))?;
    let bid_rows = sqlx::query(
        "SELECT
            b.id,
            l.auction_id,
            b.lot_id,
            l.lot_number,
            b.amount,
            b.accepted_state_version,
            b.fact_index,
            b.mechanism,
            b.created_at,
            COALESCE(l.leader_user_id = b.bidder_user_id AND l.current_price = b.amount, 0) AS leading,
            e.name AS equipment_name,
            e.category AS equipment_category
         FROM bids AS b
         JOIN lots AS l ON l.id = b.lot_id
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         JOIN equipment AS e ON e.id = l.equipment_id
         WHERE b.bidder_user_id = ?
           AND a.status IN ('live', 'final', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
         ORDER BY b.created_at DESC, b.id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(uuid_blob(user_id))
    .bind(request.bids.limit)
    .bind(request.bids.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 SQLite 用户竞价失败"))?;
    let bids = bid_rows.into_iter().map(map_sqlite_bid).collect::<Result<Vec<_>, _>>()?;

    let bid_total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)
         FROM bids AS b
         JOIN lots AS l ON l.id = b.lot_id
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         WHERE b.bidder_user_id = ?
           AND a.status IN ('live', 'final', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')",
    )
    .bind(uuid_blob(user_id))
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 SQLite 用户竞价失败"))?;

    let proxy_rows = sqlx::query(
        "SELECT
            p.authorization_id,
            l.auction_id,
            p.lot_id,
            p.user_id,
            l.lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            ) AS quota_used,
            COALESCE(CAST(json_extract(a.policy_snapshot, '$.proxy_max_outbid_uses') AS INTEGER), 0) AS quota_limit,
            json_extract(a.policy_snapshot, '$.bid_increment_ppm') AS policy_bid_increment_ppm,
            json_extract(a.policy_snapshot, '$.minimum_increment') AS policy_minimum_increment,
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
         FROM bid_proxy_authorizations AS p
         JOIN lots AS l ON l.id = p.lot_id
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         JOIN equipment AS e ON e.id = l.equipment_id
         WHERE p.user_id = ?
           AND a.status IN ('live', 'final', 'settling', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
         ORDER BY p.updated_at DESC, p.lot_id
         LIMIT ? OFFSET ?",
    )
    .bind(uuid_blob(user_id))
    .bind(request.proxies.limit)
    .bind(request.proxies.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 SQLite 用户代理历史失败"))?;

    let mut proxies = Vec::with_capacity(proxy_rows.len());
    for row in proxy_rows {
        let base = map_sqlite_proxy_base(row)?;
        let transitions = load_sqlite_transitions(&mut transaction, base.authorization_id).await?;
        let bid = load_sqlite_proxy_bid(&mut transaction, &base).await?;
        proxies.push(build_proxy(base, transitions, bid)?);
    }

    let proxy_total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)
         FROM bid_proxy_authorizations AS p
         JOIN lots AS l ON l.id = p.lot_id
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         WHERE p.user_id = ?
           AND a.status IN ('live', 'final', 'settling', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')",
    )
    .bind(uuid_blob(user_id))
    .fetch_one(&mut *transaction)
    .await
    .map_err(|_| storage_error("统计 SQLite 用户代理历史失败"))?;

    transaction.commit().await.map_err(|_| storage_error("提交 SQLite 账户读取事务失败"))?;
    let page = PublicMyBidsPage { bids, proxies, bid_total, proxy_total };
    validate_page(&page, request)
}

#[cfg(feature = "sqlite")]
pub(super) async fn proxy_sqlite(
    store: &super::SqliteStore,
    lot_id: Uuid,
    user_id: Uuid,
    authorization_id: Option<Uuid>,
) -> Result<Option<PublicOwnedProxy>, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("开启 SQLite 用户代理读取事务失败"))?;
    let row = {
        let statement = if authorization_id.is_some() {
            "SELECT
            p.authorization_id,
            l.auction_id,
            p.lot_id,
            p.user_id,
            l.lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            ) AS quota_used,
            COALESCE(CAST(json_extract(a.policy_snapshot, '$.proxy_max_outbid_uses') AS INTEGER), 0) AS quota_limit,
            json_extract(a.policy_snapshot, '$.bid_increment_ppm') AS policy_bid_increment_ppm,
            json_extract(a.policy_snapshot, '$.minimum_increment') AS policy_minimum_increment,
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
         FROM bid_proxy_authorizations AS p
         JOIN lots AS l ON l.id = p.lot_id
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         JOIN equipment AS e ON e.id = l.equipment_id
         WHERE p.lot_id = ? AND p.user_id = ? AND p.authorization_id = ?
           AND a.status IN ('live', 'final', 'settling', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
         LIMIT 1"
        } else {
            "SELECT
            p.authorization_id,
            l.auction_id,
            p.lot_id,
            p.user_id,
            l.lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            ) AS quota_used,
            COALESCE(CAST(json_extract(a.policy_snapshot, '$.proxy_max_outbid_uses') AS INTEGER), 0) AS quota_limit,
            json_extract(a.policy_snapshot, '$.bid_increment_ppm') AS policy_bid_increment_ppm,
            json_extract(a.policy_snapshot, '$.minimum_increment') AS policy_minimum_increment,
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
         FROM bid_proxy_authorizations AS p
         JOIN lots AS l ON l.id = p.lot_id
         JOIN auctions AS a ON a.id = l.auction_id
         JOIN worlds AS w ON w.id = a.world_id AND w.enabled = 1
         JOIN equipment AS e ON e.id = l.equipment_id
         WHERE p.lot_id = ? AND p.user_id = ?
           AND a.status IN ('live', 'final', 'settling', 'settled')
           AND l.status IN ('live', 'sold', 'unsold')
         ORDER BY p.usage_number DESC
         LIMIT 1"
        };
        let mut query = sqlx::query(statement).bind(uuid_blob(lot_id)).bind(uuid_blob(user_id));
        if let Some(authorization_id) = authorization_id {
            query = query.bind(uuid_blob(authorization_id));
        }
        query
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| storage_error("读取 SQLite 用户代理失败"))?
    };
    let Some(row) = row else {
        transaction.commit().await.map_err(|_| storage_error("提交 SQLite 用户代理事务失败"))?;
        return Ok(None);
    };
    let base = map_sqlite_proxy_base(row)?;
    let transitions = load_sqlite_transitions(&mut transaction, base.authorization_id).await?;
    let bid = load_sqlite_proxy_bid(&mut transaction, &base).await?;
    let proxy = build_proxy(base, transitions, bid)?;
    transaction.commit().await.map_err(|_| storage_error("提交 SQLite 用户代理事务失败"))?;
    Ok(Some(proxy))
}

#[cfg(feature = "sqlite")]
async fn load_sqlite_transitions(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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
         FROM bid_proxy_authorization_transitions
         WHERE authorization_id = ?
         ORDER BY transition_number",
    )
    .bind(uuid_blob(authorization_id))
    .fetch_all(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 SQLite 代理转换历史失败"))?;
    rows.into_iter().map(map_sqlite_transition).collect()
}

#[cfg(feature = "sqlite")]
async fn load_sqlite_proxy_bid(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    base: &ProxyBase,
) -> Result<Option<PublicProxyBid>, ApplicationError> {
    let row = sqlx::query(
        "WITH next_authorization AS (
            SELECT min(next_authorization.created_at) AS created_at
            FROM bid_proxy_authorizations AS next_authorization
            WHERE next_authorization.lot_id = ?
              AND next_authorization.user_id = ?
              AND next_authorization.usage_number = ? + 1
         )
         SELECT
            b.id,
            l.auction_id,
            b.lot_id,
            l.lot_number,
            b.amount,
            b.accepted_state_version,
            b.fact_index,
            b.mechanism,
            b.created_at,
            COALESCE(l.leader_user_id = b.bidder_user_id AND l.current_price = b.amount, 0) AS leading
         FROM bids AS b
         JOIN lots AS l ON l.id = b.lot_id
         CROSS JOIN next_authorization
         WHERE b.lot_id = ?
           AND b.bidder_user_id = ?
           AND b.mechanism = 'proxy'
           AND b.created_at >= ?
           AND (next_authorization.created_at IS NULL OR b.created_at < next_authorization.created_at)
         ORDER BY b.accepted_state_version DESC, b.fact_index DESC, b.id DESC
         LIMIT 1",
    )
    .bind(uuid_blob(base.lot_id))
    .bind(uuid_blob(base.user_id))
    .bind(base.usage_number)
    .bind(uuid_blob(base.lot_id))
    .bind(uuid_blob(base.user_id))
    .bind(base.created_at.to_rfc3339())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 SQLite 代理最近竞价失败"))?;
    row.map(map_sqlite_proxy_bid).transpose()
}
