//! Realtime Admin dashboard 的 lag 与 failure 水位快照。

use auction_application::{
    ApplicationError, MAX_REALTIME_ADMIN_PAGE_SIZE, RealtimeDashboardSnapshot,
    RealtimeFailureWater, RealtimeLagListRequest,
};
use chrono::Utc;
use sqlx::Row;

#[cfg(feature = "sqlite")]
use super::common::parse_timestamp;
use super::common::storage_error;
#[cfg(feature = "postgres")]
use super::lag::list_lag_postgres;
#[cfg(feature = "sqlite")]
use super::lag::list_lag_sqlite;
#[cfg(feature = "postgres")]
pub(super) async fn dashboard_snapshot_postgres(
    store: &super::super::PostgresStore,
) -> Result<RealtimeDashboardSnapshot, ApplicationError> {
    let lag_page = list_lag_postgres(
        store,
        RealtimeLagListRequest {
            stream_prefix: None,
            consumer_key: None,
            limit: MAX_REALTIME_ADMIN_PAGE_SIZE,
            offset: 0,
        },
    )
    .await?;
    let failure = failure_water_postgres(store).await?;
    let snapshot =
        RealtimeDashboardSnapshot { observed_at: Utc::now(), lags: lag_page.entries, failure };
    if snapshot.validate() {
        Ok(snapshot)
    } else {
        Err(storage_error("PostgreSQL realtime dashboard 摘要无效"))
    }
}

#[cfg(feature = "sqlite")]
pub(super) async fn dashboard_snapshot_sqlite(
    store: &super::super::SqliteStore,
) -> Result<RealtimeDashboardSnapshot, ApplicationError> {
    let lag_page = list_lag_sqlite(
        store,
        RealtimeLagListRequest {
            stream_prefix: None,
            consumer_key: None,
            limit: MAX_REALTIME_ADMIN_PAGE_SIZE,
            offset: 0,
        },
    )
    .await?;
    let failure = failure_water_sqlite(store).await?;
    let snapshot =
        RealtimeDashboardSnapshot { observed_at: Utc::now(), lags: lag_page.entries, failure };
    if snapshot.validate() {
        Ok(snapshot)
    } else {
        Err(storage_error("Lite realtime dashboard 摘要无效"))
    }
}

#[cfg(feature = "postgres")]
async fn failure_water_postgres(
    store: &super::super::PostgresStore,
) -> Result<RealtimeFailureWater, ApplicationError> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS count,
                MIN(failed_at) AS oldest_at,
                COALESCE((
                    SELECT latest.failure_code
                    FROM public.realtime_failures AS latest
                    WHERE latest.resolved_at IS NULL
                    ORDER BY latest.failed_at DESC, latest.id DESC
                    LIMIT 1
                ), '') AS latest_code
         FROM public.realtime_failures
         WHERE resolved_at IS NULL",
    )
    .fetch_one(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL realtime failure 水位失败"))?;
    let water = RealtimeFailureWater {
        count: row.try_get("count").map_err(|_| storage_error("读取 realtime failure 数量失败"))?,
        oldest_at: row
            .try_get("oldest_at")
            .map_err(|_| storage_error("读取 realtime failure 最早时间失败"))?,
        latest_code: row
            .try_get("latest_code")
            .map_err(|_| storage_error("读取 realtime failure 最新代码失败"))?,
    };
    if water.validate() {
        Ok(water)
    } else {
        Err(storage_error("PostgreSQL realtime failure 水位投影无效"))
    }
}

#[cfg(feature = "sqlite")]
async fn failure_water_sqlite(
    store: &super::super::SqliteStore,
) -> Result<RealtimeFailureWater, ApplicationError> {
    let row = sqlx::query(
        "SELECT COUNT(*) AS count,
                MIN(failed_at) AS oldest_at,
                COALESCE((
                    SELECT latest.failure_code
                    FROM realtime_failures AS latest
                    WHERE latest.resolved_at IS NULL
                    ORDER BY latest.failed_at DESC, latest.id DESC
                    LIMIT 1
                ), '') AS latest_code
         FROM realtime_failures
         WHERE resolved_at IS NULL",
    )
    .fetch_one(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite realtime failure 水位失败"))?;
    let water = RealtimeFailureWater {
        count: row.try_get("count").map_err(|_| storage_error("读取 Lite failure 数量失败"))?,
        oldest_at: row
            .try_get::<Option<String>, _>("oldest_at")
            .map_err(|_| storage_error("读取 Lite failure 最早时间失败"))?
            .map(parse_timestamp)
            .transpose()?,
        latest_code: row
            .try_get("latest_code")
            .map_err(|_| storage_error("读取 Lite failure 最新代码失败"))?,
    };
    if water.validate() {
        Ok(water)
    } else {
        Err(storage_error("Lite realtime failure 水位投影无效"))
    }
}
