//! Public 账户竞价与代理历史的只读适配器。
//!
//! 账户 projection 只在同一数据库读取快照内拼装：普通竞价、代理授权历史、quota 和最近
//! 一条代理事实必须属于当前用户，并且只能来自公开可见的拍卖/lot。代理的 owner capability
//! 在这里按权威 lot 状态派生，HTTP 层不接触数据库行或内部代理状态机。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicAccountReader, PublicMyBid, PublicMyBidsPage,
    PublicMyBidsRequest, PublicOwnedProxy, PublicProxyBid, PublicProxyCapabilities,
    PublicProxyTransition, RepositoryError,
};
use auction_domain::{AuctionRules, BidAmount};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

impl PublicAccountReader for DatabaseStore {
    fn list_public_user_bids(
        &self,
        user_id: Uuid,
        request: PublicMyBidsRequest,
    ) -> ApplicationFuture<'_, PublicMyBidsPage> {
        Box::pin(async move {
            validate_account_request(user_id, request)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, user_id, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store, user_id, request).await,
            }
        })
    }

    fn public_user_proxy(
        &self,
        lot_id: Uuid,
        user_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicOwnedProxy>> {
        Box::pin(async move {
            if lot_id.is_nil() || user_id.is_nil() {
                return Err(storage_error("公开账户代理查询标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    proxy_postgres(store, lot_id, user_id, None).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => proxy_sqlite(store, lot_id, user_id, None).await,
            }
        })
    }

    fn public_user_proxy_by_id(
        &self,
        lot_id: Uuid,
        authorization_id: Uuid,
        user_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicOwnedProxy>> {
        Box::pin(async move {
            if lot_id.is_nil() || authorization_id.is_nil() || user_id.is_nil() {
                return Err(storage_error("公开账户历史代理查询标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    proxy_postgres(store, lot_id, user_id, Some(authorization_id)).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    proxy_sqlite(store, lot_id, user_id, Some(authorization_id)).await
                }
            }
        })
    }
}

#[derive(Debug)]
struct ProxyBase {
    authorization_id: Uuid,
    auction_id: Uuid,
    lot_id: Uuid,
    user_id: Uuid,
    lot_number: i64,
    authorization_sequence: i64,
    usage_number: i64,
    quota_used: i64,
    quota_limit: i64,
    mode: String,
    maximum_amount: i64,
    status: String,
    terminal_reason: Option<String>,
    state_version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    equipment_name: String,
    equipment_category: String,
    start_price: i64,
    current_price: Option<i64>,
    material: bool,
    leader_user_id: Option<Uuid>,
    slow_authorization_id: Option<Uuid>,
}

#[cfg(feature = "postgres")]
async fn list_postgres(
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
            l.lot_number,
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
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
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
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
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
            l.lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM public.bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            )::bigint AS quota_used,
            COALESCE((a.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0) AS quota_limit,
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
         JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
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
async fn proxy_postgres(
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
            l.lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM public.bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            )::bigint AS quota_used,
            COALESCE((a.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0) AS quota_limit,
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
            l.lot_number,
            p.authorization_sequence,
            p.usage_number,
            (
                SELECT count(*)
                FROM public.bid_proxy_authorizations AS quota
                WHERE quota.lot_id = p.lot_id AND quota.user_id = p.user_id
            )::bigint AS quota_used,
            COALESCE((a.policy_snapshot ->> 'proxy_max_outbid_uses')::bigint, 0) AS quota_limit,
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
            l.lot_number,
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

#[cfg(feature = "sqlite")]
async fn list_sqlite(
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
async fn proxy_sqlite(
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

#[cfg(feature = "postgres")]
fn map_postgres_bid(row: sqlx::postgres::PgRow) -> Result<PublicMyBid, ApplicationError> {
    let bid = PublicMyBid {
        id: get_pg(&row, "id", "读取 PostgreSQL 用户竞价标识失败")?,
        auction_id: get_pg(&row, "auction_id", "读取 PostgreSQL 用户竞价拍卖标识失败")?,
        lot_id: get_pg(&row, "lot_id", "读取 PostgreSQL 用户竞价 lot 标识失败")?,
        lot_number: i64::from(get_pg::<i32>(
            &row,
            "lot_number",
            "读取 PostgreSQL 用户竞价序号失败",
        )?),
        amount: get_pg(&row, "amount", "读取 PostgreSQL 用户竞价金额失败")?,
        accepted_state_version: get_pg(
            &row,
            "accepted_state_version",
            "读取 PostgreSQL 用户竞价状态版本失败",
        )?,
        fact_index: get_pg(&row, "fact_index", "读取 PostgreSQL 用户竞价事实序号失败")?,
        mechanism: get_pg(&row, "mechanism", "读取 PostgreSQL 用户竞价机制失败")?,
        created_at: get_pg(&row, "created_at", "读取 PostgreSQL 用户竞价时间失败")?,
        equipment_name: get_pg(&row, "equipment_name", "读取 PostgreSQL 用户竞价装备名称失败")?,
        equipment_category: get_pg(
            &row,
            "equipment_category",
            "读取 PostgreSQL 用户竞价装备分类失败",
        )?,
        leading: get_pg(&row, "leading", "读取 PostgreSQL 用户竞价领先状态失败")?,
    };
    validate_my_bid(bid)
}

#[cfg(feature = "sqlite")]
fn map_sqlite_bid(row: sqlx::sqlite::SqliteRow) -> Result<PublicMyBid, ApplicationError> {
    let bid = PublicMyBid {
        id: get_sqlite_uuid(&row, "id", "读取 SQLite 用户竞价标识失败")?,
        auction_id: get_sqlite_uuid(&row, "auction_id", "读取 SQLite 用户竞价拍卖标识失败")?,
        lot_id: get_sqlite_uuid(&row, "lot_id", "读取 SQLite 用户竞价 lot 标识失败")?,
        lot_number: get_sqlite(&row, "lot_number", "读取 SQLite 用户竞价序号失败")?,
        amount: get_sqlite(&row, "amount", "读取 SQLite 用户竞价金额失败")?,
        accepted_state_version: get_sqlite(
            &row,
            "accepted_state_version",
            "读取 SQLite 用户竞价状态版本失败",
        )?,
        fact_index: i32::try_from(get_sqlite::<i64>(
            &row,
            "fact_index",
            "读取 SQLite 用户竞价事实序号失败",
        )?)
        .map_err(|_| storage_error("SQLite 用户竞价事实序号超出范围"))?,
        mechanism: get_sqlite(&row, "mechanism", "读取 SQLite 用户竞价机制失败")?,
        created_at: parse_timestamp(
            get_sqlite(&row, "created_at", "读取 SQLite 用户竞价时间失败")?,
            "SQLite 用户竞价时间格式无效",
        )?,
        equipment_name: get_sqlite(&row, "equipment_name", "读取 SQLite 用户竞价装备名称失败")?,
        equipment_category: get_sqlite(
            &row,
            "equipment_category",
            "读取 SQLite 用户竞价装备分类失败",
        )?,
        leading: get_sqlite::<i64>(&row, "leading", "读取 SQLite 用户竞价领先状态失败")? != 0,
    };
    validate_my_bid(bid)
}

#[cfg(feature = "postgres")]
fn map_postgres_proxy_base(row: sqlx::postgres::PgRow) -> Result<ProxyBase, ApplicationError> {
    Ok(ProxyBase {
        authorization_id: get_pg(&row, "authorization_id", "读取 PostgreSQL 代理授权标识失败")?,
        auction_id: get_pg(&row, "auction_id", "读取 PostgreSQL 代理拍卖标识失败")?,
        lot_id: get_pg(&row, "lot_id", "读取 PostgreSQL 代理 lot 标识失败")?,
        user_id: get_pg(&row, "user_id", "读取 PostgreSQL 代理用户标识失败")?,
        lot_number: i64::from(get_pg::<i32>(
            &row,
            "lot_number",
            "读取 PostgreSQL 代理 lot 序号失败",
        )?),
        authorization_sequence: get_pg(
            &row,
            "authorization_sequence",
            "读取 PostgreSQL 代理授权序号失败",
        )?,
        usage_number: get_pg(&row, "usage_number", "读取 PostgreSQL 代理使用序号失败")?,
        quota_used: get_pg(&row, "quota_used", "读取 PostgreSQL 代理 quota 使用量失败")?,
        quota_limit: get_pg(&row, "quota_limit", "读取 PostgreSQL 代理 quota 上限失败")?,
        mode: get_pg(&row, "mode", "读取 PostgreSQL 代理模式失败")?,
        maximum_amount: get_pg(&row, "maximum_amount", "读取 PostgreSQL 代理金额失败")?,
        status: get_pg(&row, "proxy_status", "读取 PostgreSQL 代理状态失败")?,
        terminal_reason: get_pg(&row, "terminal_reason", "读取 PostgreSQL 代理终止原因失败")?,
        state_version: get_pg(&row, "state_version", "读取 PostgreSQL 代理状态版本失败")?,
        created_at: get_pg(&row, "created_at", "读取 PostgreSQL 代理创建时间失败")?,
        updated_at: get_pg(&row, "updated_at", "读取 PostgreSQL 代理更新时间失败")?,
        equipment_name: get_pg(&row, "equipment_name", "读取 PostgreSQL 代理装备名称失败")?,
        equipment_category: get_pg(&row, "equipment_category", "读取 PostgreSQL 代理装备分类失败")?,
        start_price: get_pg(&row, "start_price", "读取 PostgreSQL 代理起始价失败")?,
        current_price: get_pg(&row, "current_price", "读取 PostgreSQL 代理当前价失败")?,
        material: get_pg(&row, "material", "读取 PostgreSQL 代理 material 标志失败")?,
        leader_user_id: get_pg(&row, "leader_user_id", "读取 PostgreSQL 代理领先用户失败")?,
        slow_authorization_id: get_pg(
            &row,
            "slow_authorization_id",
            "读取 PostgreSQL 代理等待授权失败",
        )?,
    })
}

#[cfg(feature = "sqlite")]
fn map_sqlite_proxy_base(row: sqlx::sqlite::SqliteRow) -> Result<ProxyBase, ApplicationError> {
    Ok(ProxyBase {
        authorization_id: get_sqlite_uuid(
            &row,
            "authorization_id",
            "读取 SQLite 代理授权标识失败",
        )?,
        auction_id: get_sqlite_uuid(&row, "auction_id", "读取 SQLite 代理拍卖标识失败")?,
        lot_id: get_sqlite_uuid(&row, "lot_id", "读取 SQLite 代理 lot 标识失败")?,
        user_id: get_sqlite_uuid(&row, "user_id", "读取 SQLite 代理用户标识失败")?,
        lot_number: get_sqlite(&row, "lot_number", "读取 SQLite 代理 lot 序号失败")?,
        authorization_sequence: get_sqlite(
            &row,
            "authorization_sequence",
            "读取 SQLite 代理授权序号失败",
        )?,
        usage_number: get_sqlite(&row, "usage_number", "读取 SQLite 代理使用序号失败")?,
        quota_used: get_sqlite(&row, "quota_used", "读取 SQLite 代理 quota 使用量失败")?,
        quota_limit: get_sqlite(&row, "quota_limit", "读取 SQLite 代理 quota 上限失败")?,
        mode: get_sqlite(&row, "mode", "读取 SQLite 代理模式失败")?,
        maximum_amount: get_sqlite(&row, "maximum_amount", "读取 SQLite 代理金额失败")?,
        status: get_sqlite(&row, "proxy_status", "读取 SQLite 代理状态失败")?,
        terminal_reason: get_sqlite(&row, "terminal_reason", "读取 SQLite 代理终止原因失败")?,
        state_version: get_sqlite(&row, "state_version", "读取 SQLite 代理状态版本失败")?,
        created_at: parse_timestamp(
            get_sqlite(&row, "created_at", "读取 SQLite 代理创建时间失败")?,
            "SQLite 代理创建时间格式无效",
        )?,
        updated_at: parse_timestamp(
            get_sqlite(&row, "updated_at", "读取 SQLite 代理更新时间失败")?,
            "SQLite 代理更新时间格式无效",
        )?,
        equipment_name: get_sqlite(&row, "equipment_name", "读取 SQLite 代理装备名称失败")?,
        equipment_category: get_sqlite(&row, "equipment_category", "读取 SQLite 代理装备分类失败")?,
        start_price: get_sqlite(&row, "start_price", "读取 SQLite 代理起始价失败")?,
        current_price: get_sqlite(&row, "current_price", "读取 SQLite 代理当前价失败")?,
        material: get_sqlite::<i64>(&row, "material", "读取 SQLite 代理 material 标志失败")? != 0,
        leader_user_id: get_sqlite_optional_uuid(
            &row,
            "leader_user_id",
            "读取 SQLite 代理领先用户失败",
        )?,
        slow_authorization_id: get_sqlite_optional_uuid(
            &row,
            "slow_authorization_id",
            "读取 SQLite 代理等待授权失败",
        )?,
    })
}

#[cfg(feature = "postgres")]
fn map_postgres_transition(
    row: sqlx::postgres::PgRow,
) -> Result<PublicProxyTransition, ApplicationError> {
    let transition = PublicProxyTransition {
        transition_number: get_pg(&row, "transition_number", "读取 PostgreSQL 代理转换序号失败")?,
        kind: get_pg(&row, "kind", "读取 PostgreSQL 代理转换类型失败")?,
        old_maximum_amount: get_pg(&row, "old_maximum_amount", "读取 PostgreSQL 代理旧上限失败")?,
        new_maximum_amount: get_pg(&row, "new_maximum_amount", "读取 PostgreSQL 代理新上限失败")?,
        old_mode: get_pg(&row, "old_mode", "读取 PostgreSQL 代理旧模式失败")?,
        new_mode: get_pg(&row, "new_mode", "读取 PostgreSQL 代理新模式失败")?,
        old_status: get_pg(&row, "old_status", "读取 PostgreSQL 代理旧状态失败")?,
        new_status: get_pg(&row, "new_status", "读取 PostgreSQL 代理新状态失败")?,
        reason: get_pg(&row, "reason", "读取 PostgreSQL 代理转换原因失败")?,
        resulting_state_version: get_pg(
            &row,
            "resulting_state_version",
            "读取 PostgreSQL 代理转换状态版本失败",
        )?,
        created_at: get_pg(&row, "created_at", "读取 PostgreSQL 代理转换时间失败")?,
    };
    normalize_transition(transition)
}

#[cfg(feature = "sqlite")]
fn map_sqlite_transition(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PublicProxyTransition, ApplicationError> {
    let transition = PublicProxyTransition {
        transition_number: get_sqlite(&row, "transition_number", "读取 SQLite 代理转换序号失败")?,
        kind: get_sqlite(&row, "kind", "读取 SQLite 代理转换类型失败")?,
        old_maximum_amount: get_sqlite(&row, "old_maximum_amount", "读取 SQLite 代理旧上限失败")?,
        new_maximum_amount: get_sqlite(&row, "new_maximum_amount", "读取 SQLite 代理新上限失败")?,
        old_mode: get_sqlite(&row, "old_mode", "读取 SQLite 代理旧模式失败")?,
        new_mode: get_sqlite(&row, "new_mode", "读取 SQLite 代理新模式失败")?,
        old_status: get_sqlite(&row, "old_status", "读取 SQLite 代理旧状态失败")?,
        new_status: get_sqlite(&row, "new_status", "读取 SQLite 代理新状态失败")?,
        reason: get_sqlite(&row, "reason", "读取 SQLite 代理转换原因失败")?,
        resulting_state_version: get_sqlite(
            &row,
            "resulting_state_version",
            "读取 SQLite 代理转换状态版本失败",
        )?,
        created_at: parse_timestamp(
            get_sqlite(&row, "created_at", "读取 SQLite 代理转换时间失败")?,
            "SQLite 代理转换时间格式无效",
        )?,
    };
    normalize_transition(transition)
}

fn normalize_transition(
    mut transition: PublicProxyTransition,
) -> Result<PublicProxyTransition, ApplicationError> {
    if transition.transition_number <= 0
        || transition.new_maximum_amount <= 0
        || transition.resulting_state_version <= 0
        || transition.kind.trim().is_empty()
        || transition.new_mode.trim().is_empty()
        || transition.new_status.trim().is_empty()
        || transition.created_at.timestamp_micros() == 0
    {
        return Err(storage_error("代理转换历史包含无效字段"));
    }
    transition.new_status = public_status(&transition.new_status)?;
    if let Some(status) = transition.old_status.as_mut() {
        *status = public_status(status)?;
    }
    Ok(transition)
}

#[cfg(feature = "postgres")]
fn map_postgres_proxy_bid(row: sqlx::postgres::PgRow) -> Result<PublicProxyBid, ApplicationError> {
    let bid = PublicProxyBid {
        id: get_pg(&row, "id", "读取 PostgreSQL 代理竞价标识失败")?,
        auction_id: get_pg(&row, "auction_id", "读取 PostgreSQL 代理竞价拍卖标识失败")?,
        lot_id: get_pg(&row, "lot_id", "读取 PostgreSQL 代理竞价 lot 标识失败")?,
        lot_number: i64::from(get_pg::<i32>(
            &row,
            "lot_number",
            "读取 PostgreSQL 代理竞价序号失败",
        )?),
        amount: get_pg(&row, "amount", "读取 PostgreSQL 代理竞价金额失败")?,
        accepted_state_version: get_pg(
            &row,
            "accepted_state_version",
            "读取 PostgreSQL 代理竞价状态版本失败",
        )?,
        fact_index: get_pg(&row, "fact_index", "读取 PostgreSQL 代理竞价事实序号失败")?,
        mechanism: get_pg(&row, "mechanism", "读取 PostgreSQL 代理竞价机制失败")?,
        created_at: get_pg(&row, "created_at", "读取 PostgreSQL 代理竞价时间失败")?,
        leading: get_pg(&row, "leading", "读取 PostgreSQL 代理竞价领先状态失败")?,
    };
    validate_proxy_bid(bid)
}

#[cfg(feature = "sqlite")]
fn map_sqlite_proxy_bid(row: sqlx::sqlite::SqliteRow) -> Result<PublicProxyBid, ApplicationError> {
    let bid = PublicProxyBid {
        id: get_sqlite_uuid(&row, "id", "读取 SQLite 代理竞价标识失败")?,
        auction_id: get_sqlite_uuid(&row, "auction_id", "读取 SQLite 代理竞价拍卖标识失败")?,
        lot_id: get_sqlite_uuid(&row, "lot_id", "读取 SQLite 代理竞价 lot 标识失败")?,
        lot_number: get_sqlite(&row, "lot_number", "读取 SQLite 代理竞价序号失败")?,
        amount: get_sqlite(&row, "amount", "读取 SQLite 代理竞价金额失败")?,
        accepted_state_version: get_sqlite(
            &row,
            "accepted_state_version",
            "读取 SQLite 代理竞价状态版本失败",
        )?,
        fact_index: i32::try_from(get_sqlite::<i64>(
            &row,
            "fact_index",
            "读取 SQLite 代理竞价事实序号失败",
        )?)
        .map_err(|_| storage_error("SQLite 代理竞价事实序号超出范围"))?,
        mechanism: get_sqlite(&row, "mechanism", "读取 SQLite 代理竞价机制失败")?,
        created_at: parse_timestamp(
            get_sqlite(&row, "created_at", "读取 SQLite 代理竞价时间失败")?,
            "SQLite 代理竞价时间格式无效",
        )?,
        leading: get_sqlite::<i64>(&row, "leading", "读取 SQLite 代理竞价领先状态失败")? != 0,
    };
    validate_proxy_bid(bid)
}

fn build_proxy(
    base: ProxyBase,
    transitions: Vec<PublicProxyTransition>,
    bid: Option<PublicProxyBid>,
) -> Result<PublicOwnedProxy, ApplicationError> {
    if base.authorization_id.is_nil()
        || base.auction_id.is_nil()
        || base.lot_id.is_nil()
        || base.user_id.is_nil()
        || base.lot_number <= 0
        || base.authorization_sequence <= 0
        || base.usage_number <= 0
        || base.quota_used <= 0
        || base.quota_limit < 0
        || base.quota_used < base.usage_number
        || base.quota_limit > 0 && base.quota_used > base.quota_limit
        || base.maximum_amount <= 0
        || base.start_price <= 0
        || base.state_version <= 0
        || base.updated_at < base.created_at
        || base.equipment_name.trim().is_empty()
        || base.equipment_category.trim().is_empty()
        || transitions.len() as i64 != base.state_version
    {
        return Err(storage_error("账户代理投影包含无效快照"));
    }
    for (index, transition) in transitions.iter().enumerate() {
        let expected = i64::try_from(index + 1).map_err(|_| storage_error("代理转换序号溢出"))?;
        if transition.transition_number != expected
            || transition.resulting_state_version != expected
        {
            return Err(storage_error("账户代理转换历史版本不连续"));
        }
    }
    if base.mode != "fast" && base.mode != "slow" {
        return Err(storage_error("账户代理模式不在公开闭集内"));
    }
    let public_status = public_status(&base.status)?;
    let active = base.status == "active" || base.status == "final_fast";
    let invalid_reason = match public_status.as_str() {
        "exhausted" => Some("outbid".to_owned()),
        "frozen" => Some("frozen".to_owned()),
        "closed" => Some("closed".to_owned()),
        "active" => None,
        _ => return Err(storage_error("账户代理状态映射失败")),
    };
    let capabilities = if !active {
        PublicProxyCapabilities { can_lower: false, waiting: false, at_maximum: false }
    } else {
        derive_capabilities(&base)?
    };
    if let Some(bid) = &bid
        && (bid.auction_id != base.auction_id || bid.lot_id != base.lot_id)
    {
        return Err(storage_error("账户代理最近竞价与 lot 不匹配"));
    }
    Ok(PublicOwnedProxy {
        authorization_id: base.authorization_id,
        auction_id: base.auction_id,
        lot_id: base.lot_id,
        user_id: base.user_id,
        lot_number: base.lot_number,
        authorization_sequence: base.authorization_sequence,
        usage_number: base.usage_number,
        quota_used: base.quota_used,
        quota_limit: base.quota_limit,
        mode: base.mode,
        maximum_amount: base.maximum_amount,
        status: public_status,
        terminal_reason: base.terminal_reason,
        state_version: base.state_version,
        transitions,
        created_at: base.created_at,
        updated_at: base.updated_at,
        equipment_name: base.equipment_name,
        equipment_category: base.equipment_category,
        bid,
        invalid_reason,
        capabilities,
    })
}

fn derive_capabilities(base: &ProxyBase) -> Result<PublicProxyCapabilities, ApplicationError> {
    let leading = base.leader_user_id == Some(base.user_id);
    let waiting = base.slow_authorization_id == Some(base.authorization_id);
    if leading == waiting
        || leading && base.slow_authorization_id.is_some()
        || waiting && (base.mode != "slow" || base.leader_user_id.is_none())
    {
        return Err(storage_error("账户代理领先/等待状态不一致"));
    }
    let current_price = base.current_price.unwrap_or(base.start_price);
    let current =
        BidAmount::new(current_price).map_err(|_| storage_error("账户代理当前金额无效"))?;
    let start =
        BidAmount::new(base.start_price).map_err(|_| storage_error("账户代理起始金额无效"))?;
    let binding = if waiting {
        AuctionRules::default().next_minimum_bid(Some(start), Some(current), base.material, false)
    } else {
        current
    };
    if base.maximum_amount < binding.value() {
        return Err(storage_error("账户代理上限低于当前合法绑定金额"));
    }
    Ok(PublicProxyCapabilities {
        can_lower: base.maximum_amount > binding.value(),
        waiting,
        at_maximum: base.maximum_amount == current.value(),
    })
}

fn public_status(status: &str) -> Result<String, ApplicationError> {
    match status {
        "active" | "final_fast" => Ok("active".to_owned()),
        "exhausted" => Ok("exhausted".to_owned()),
        "frozen" => Ok("frozen".to_owned()),
        "closed" => Ok("closed".to_owned()),
        _ => Err(storage_error("账户代理状态未知")),
    }
}

fn validate_page(
    page: &PublicMyBidsPage,
    request: PublicMyBidsRequest,
) -> Result<PublicMyBidsPage, ApplicationError> {
    if page.bid_total < 0
        || page.proxy_total < 0
        || page.bids.len() as i64 > request.bids.limit
        || page.proxies.len() as i64 > request.proxies.limit
        || page.bids.len() as i64 > page.bid_total
        || page.proxies.len() as i64 > page.proxy_total
        || page.proxies.iter().any(|proxy| proxy.user_id.is_nil())
    {
        return Err(storage_error("账户竞价页结果不满足边界约束"));
    }
    Ok(page.clone())
}

fn validate_account_request(
    user_id: Uuid,
    request: PublicMyBidsRequest,
) -> Result<(), ApplicationError> {
    if user_id.is_nil() {
        return Err(storage_error("公开账户用户标识无效"));
    }
    request.validate().map_err(storage_error)
}

fn validate_my_bid(bid: PublicMyBid) -> Result<PublicMyBid, ApplicationError> {
    if bid.id.is_nil()
        || bid.auction_id.is_nil()
        || bid.lot_id.is_nil()
        || bid.lot_number <= 0
        || bid.amount <= 0
        || bid.accepted_state_version <= 0
        || bid.fact_index <= 0
        || (bid.mechanism != "ordinary" && bid.mechanism != "proxy")
        || bid.created_at.timestamp_micros() == 0
        || bid.equipment_name.trim().is_empty()
        || bid.equipment_category.trim().is_empty()
    {
        return Err(storage_error("账户竞价投影包含无效事实"));
    }
    Ok(bid)
}

fn validate_proxy_bid(bid: PublicProxyBid) -> Result<PublicProxyBid, ApplicationError> {
    if bid.id.is_nil()
        || bid.auction_id.is_nil()
        || bid.lot_id.is_nil()
        || bid.lot_number <= 0
        || bid.amount <= 0
        || bid.accepted_state_version <= 0
        || bid.fact_index <= 0
        || bid.mechanism != "proxy"
        || bid.created_at.timestamp_micros() == 0
    {
        return Err(storage_error("账户代理竞价投影包含无效事实"));
    }
    Ok(bid)
}

#[cfg(feature = "postgres")]
fn get_pg<T>(row: &sqlx::postgres::PgRow, column: &str, label: &str) -> Result<T, ApplicationError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn get_sqlite<T>(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    label: &str,
) -> Result<T, ApplicationError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(column).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn get_sqlite_uuid(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    label: &str,
) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&get_sqlite::<Vec<u8>>(row, column, label)?).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn get_sqlite_optional_uuid(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
    label: &str,
) -> Result<Option<Uuid>, ApplicationError> {
    get_sqlite::<Option<Vec<u8>>>(row, column, label)?
        .map(|bytes| Uuid::from_slice(&bytes).map_err(|_| storage_error(label)))
        .transpose()
}

#[cfg(feature = "sqlite")]
fn uuid_blob(id: Uuid) -> Vec<u8> {
    id.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String, label: &str) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error(label))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
