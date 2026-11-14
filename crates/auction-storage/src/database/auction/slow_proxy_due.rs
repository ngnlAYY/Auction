//! 慢速代理持久化到期扫描适配器；只读 lot 上的 due generation 并用数据库时钟筛选，不领取内存
//! 任务、不提前持锁，已过竞价截止的候选在 LIMIT 前排除，避免 Final 失败时反复占满批次。
//! 真正执行必须回到 PostgreSQL 竞价 Serializable 事务重新核对全部 fence。

use auction_application::auction::{ApplicationFuture, SlowProxyDue, SlowProxyDueReader};
use auction_application::core::ApplicationError;
use auction_domain::{LotId, ProxyAuthorizationId};
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::storage_error;
use crate::database::storage_error_from;

impl SlowProxyDueReader for DatabaseStore {
    fn list_due_slow_proxies(&self, limit: u16) -> ApplicationFuture<'_, Vec<SlowProxyDue>> {
        Box::pin(async move {
            if limit == 0 || limit > 1000 {
                return Err(storage_error("慢速代理扫描上限必须在 1..=1000"));
            }
            list_postgres(self, limit).await
        })
    }

    fn next_slow_proxy_due_at(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<DateTime<Utc>>> {
        Box::pin(async move { next_due_postgres(self, now).await })
    }
}

/// 唤醒提示沿用扫描的 live 谓词与 `lots_due_slow_idx`；截止后才到期的调度永远不会执行，提前排除。
/// 只返回严格晚于 `now` 的到期时刻，已到期候选仍由 `list_due_slow_proxies` 按常规节奏扫描。
async fn next_due_postgres(
    store: &crate::database::DatabaseStore,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, ApplicationError> {
    sqlx::query_scalar::<_, DateTime<Utc>>(
        "SELECT lot.slow_due_at
         FROM public.lots AS lot
         JOIN public.auctions AS auction ON auction.id = lot.auction_id
         WHERE lot.status = 'live'
           AND auction.status = 'live'
           AND lot.slow_due_at > $1
           AND lot.effective_end_at > lot.slow_due_at
         ORDER BY lot.slow_due_at, lot.id
         LIMIT 1",
    )
    .bind(now)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL 慢速代理下次到期失败"))
}

async fn list_postgres(
    store: &crate::database::DatabaseStore,
    limit: u16,
) -> Result<Vec<SlowProxyDue>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT
            lot.id AS lot_id,
            lot.slow_authorization_id AS authorization_id,
            lot.slow_generation,
            lot.slow_due_at AS due_at
         FROM public.lots AS lot
         JOIN public.auctions AS auction ON auction.id = lot.auction_id
         JOIN public.bid_proxy_authorizations AS proxy
           ON proxy.authorization_id = lot.slow_authorization_id
          AND proxy.lot_id = lot.id
          AND proxy.status = 'active'
          AND proxy.mode = 'slow'
         WHERE lot.status = 'live'
           AND auction.status = 'live'
           AND lot.slow_due_at <= statement_timestamp()
           AND lot.effective_end_at > statement_timestamp()
         ORDER BY lot.slow_due_at, lot.id
         LIMIT $1",
    )
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "扫描 PostgreSQL 慢速代理失败"))?;

    rows.into_iter().map(map_postgres_row).collect()
}

fn map_postgres_row(row: sqlx::postgres::PgRow) -> Result<SlowProxyDue, ApplicationError> {
    let lot_id: Uuid = row
        .try_get("lot_id")
        .map_err(|error| storage_error_from(error, "慢速代理 lot 标识损坏"))?;
    let authorization_id: Uuid = row
        .try_get("authorization_id")
        .map_err(|error| storage_error_from(error, "慢速代理授权标识损坏"))?;
    let generation: i64 = row
        .try_get("slow_generation")
        .map_err(|error| storage_error_from(error, "慢速代理世代损坏"))?;
    let due_at =
        row.try_get("due_at").map_err(|error| storage_error_from(error, "慢速代理到期时间损坏"))?;
    let due = SlowProxyDue {
        lot_id: LotId::from_uuid(lot_id),
        authorization_id: ProxyAuthorizationId::from_uuid(authorization_id),
        generation: positive_generation(generation)?,
        due_at,
    };
    due.validate().map_err(|_| storage_error("PostgreSQL 慢速代理扫描结果损坏"))?;
    Ok(due)
}

fn positive_generation(value: i64) -> Result<u64, ApplicationError> {
    if value <= 0 {
        return Err(storage_error("慢速代理世代必须为正数"));
    }
    u64::try_from(value).map_err(|_| storage_error("慢速代理世代超出领域范围"))
}
