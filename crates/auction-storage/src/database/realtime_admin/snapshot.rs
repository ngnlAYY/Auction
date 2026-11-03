//! Realtime Admin dashboard 的 lag 与 failure 水位快照。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::{
    ApplicationError, MAX_REALTIME_ADMIN_PAGE_SIZE, RealtimeDashboardSnapshot,
    RealtimeFailureWater, RealtimeLagListRequest,
};
use chrono::Utc;
use sqlx::Row;

use super::common::storage_error;
#[cfg(feature = "postgres")]
use super::lag::read_lag_entries_postgres;
#[cfg(feature = "postgres")]
pub(super) async fn dashboard_snapshot_postgres(
    store: &super::super::PostgresStore,
) -> Result<RealtimeDashboardSnapshot, ApplicationError> {
    // 全量积压、有界明细和失败水位共用只读快照；第 201 条之后的 lag 也必须影响健康。
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启 realtime dashboard 快照失败"))?;
    let total_lag = total_lag_postgres(&mut transaction).await?;
    let lags = read_lag_entries_postgres(
        &mut transaction,
        &RealtimeLagListRequest {
            stream_prefix: None,
            consumer_key: None,
            limit: MAX_REALTIME_ADMIN_PAGE_SIZE,
            offset: 0,
        },
    )
    .await?;
    let failure = failure_water_postgres(&mut transaction).await?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 realtime dashboard 快照失败"))?;
    let snapshot = RealtimeDashboardSnapshot { observed_at: Utc::now(), total_lag, lags, failure };
    if snapshot.validate() {
        Ok(snapshot)
    } else {
        Err(storage_error("PostgreSQL realtime dashboard 摘要无效"))
    }
}

#[cfg(feature = "postgres")]
async fn total_lag_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
) -> Result<i64, ApplicationError> {
    // SUM(bigint) 使用 numeric 累加，再严格转换为 i64；溢出或任一水位倒退均失败关闭。
    let row = sqlx::query(
        "SELECT COALESCE(SUM(stream.next_sequence - 1 - offset_row.last_sequence), 0)::bigint
                    AS total_lag,
                COALESCE(BOOL_AND(offset_row.last_sequence >= 0
                    AND offset_row.last_sequence < stream.next_sequence), true) AS valid
         FROM public.realtime_consumer_offsets AS offset_row
         JOIN public.realtime_streams AS stream ON stream.stream_key = offset_row.stream_key",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime 全量 lag 失败"))?;
    if !row
        .try_get::<bool, _>("valid")
        .map_err(|error| storage_error_from(error, "读取 realtime lag 有效性失败"))?
    {
        return Err(storage_error("PostgreSQL realtime lag 水位无效"));
    }
    row.try_get("total_lag")
        .map_err(|error| storage_error_from(error, "读取 realtime 全量 lag 数值失败"))
}

#[cfg(feature = "postgres")]
async fn failure_water_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
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
    .fetch_one(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime failure 水位失败"))?;
    let water = RealtimeFailureWater {
        count: row
            .try_get("count")
            .map_err(|error| storage_error_from(error, "读取 realtime failure 数量失败"))?,
        oldest_at: row
            .try_get("oldest_at")
            .map_err(|error| storage_error_from(error, "读取 realtime failure 最早时间失败"))?,
        latest_code: row
            .try_get("latest_code")
            .map_err(|error| storage_error_from(error, "读取 realtime failure 最新代码失败"))?,
    };
    if water.validate() {
        Ok(water)
    } else {
        Err(storage_error("PostgreSQL realtime failure 水位投影无效"))
    }
}
