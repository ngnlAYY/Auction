//! 慢速代理持久化到期扫描适配器。
//!
//! 扫描只读取 lot 上的 due generation，并使用数据库时钟筛选；它不领取内存任务，也不提前持有
//! 业务锁。已过竞价截止的候选在 LIMIT 前排除，避免 Final 推进失败时反复占满批次。
//! 真正执行必须回到竞价 Serializable/`BEGIN IMMEDIATE` 事务重新核对全部 fence。

use auction_application::{
    ApplicationError, ApplicationFuture, RepositoryError, SlowProxyDue, SlowProxyDueReader,
};
use auction_domain::{LotId, ProxyAuthorizationId};
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

impl SlowProxyDueReader for DatabaseStore {
    fn list_due_slow_proxies(&self, limit: u16) -> ApplicationFuture<'_, Vec<SlowProxyDue>> {
        Box::pin(async move {
            if limit == 0 || limit > 1000 {
                return Err(storage_error("慢速代理扫描上限必须在 1..=1000"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_postgres(store, limit).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
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
    .map_err(|_| storage_error("扫描 PostgreSQL 慢速代理失败"))?;

    rows.into_iter().map(map_postgres_row).collect()
}

#[cfg(feature = "postgres")]
fn map_postgres_row(row: sqlx::postgres::PgRow) -> Result<SlowProxyDue, ApplicationError> {
    let lot_id: Uuid = row.try_get("lot_id").map_err(|_| storage_error("慢速代理 lot 标识损坏"))?;
    let authorization_id: Uuid =
        row.try_get("authorization_id").map_err(|_| storage_error("慢速代理授权标识损坏"))?;
    let generation: i64 =
        row.try_get("slow_generation").map_err(|_| storage_error("慢速代理世代损坏"))?;
    let due_at = row.try_get("due_at").map_err(|_| storage_error("慢速代理到期时间损坏"))?;
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

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
