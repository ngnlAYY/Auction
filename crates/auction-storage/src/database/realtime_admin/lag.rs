//! Realtime consumer lag 的分页查询与安全投影。

use crate::database::storage_error_from;

use auction_application::core::ApplicationError;
use auction_application::realtime::{RealtimeLagListRequest, RealtimeLagPage, RealtimeLagSnapshot};
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::common::storage_error;
pub(super) async fn list_lag_postgres(
    store: &super::super::DatabaseStore,
    request: RealtimeLagListRequest,
) -> Result<RealtimeLagPage, ApplicationError> {
    // consumer 可在读取期间登记新 stream；总数与水位页面共用只读快照。
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启 realtime lag 快照失败"))?;
    let stream_prefix = request.stream_prefix.as_deref();
    let consumer_key = request.consumer_key.as_deref();
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM public.realtime_consumer_offsets AS offset_row
         JOIN public.realtime_streams AS stream ON stream.stream_key = offset_row.stream_key
         WHERE offset_row.stream_key LIKE COALESCE($1::text || '%', '%')
           AND offset_row.consumer_key = COALESCE($2::text, offset_row.consumer_key)",
    )
    .bind(stream_prefix)
    .bind(consumer_key)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime lag 总数失败"))?;
    let entries = read_lag_entries_postgres(&mut transaction, &request).await?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 realtime lag 快照失败"))?;
    validate_lag_page(&entries, total, &request)?;
    Ok(RealtimeLagPage { entries, total, limit: request.limit, offset: request.offset })
}

/// 有界明细复用调用方快照，避免 dashboard 全量积压与列表观察不同的水位。
pub(super) async fn read_lag_entries_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    request: &RealtimeLagListRequest,
) -> Result<Vec<RealtimeLagSnapshot>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT offset_row.consumer_key, offset_row.stream_key,
                offset_row.last_sequence, stream.next_sequence - 1 AS latest_sequence,
                offset_row.updated_at AS last_advanced_at
         FROM public.realtime_consumer_offsets AS offset_row
         JOIN public.realtime_streams AS stream ON stream.stream_key = offset_row.stream_key
         WHERE offset_row.stream_key LIKE COALESCE($1::text || '%', '%')
           AND offset_row.consumer_key = COALESCE($2::text, offset_row.consumer_key)
         ORDER BY offset_row.consumer_key ASC, offset_row.stream_key ASC
         LIMIT $3 OFFSET $4",
    )
    .bind(request.stream_prefix.as_deref())
    .bind(request.consumer_key.as_deref())
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime lag 失败"))?;
    rows.into_iter().map(lag_from_postgres_row).collect()
}

fn lag_from_postgres_row(row: PgRow) -> Result<RealtimeLagSnapshot, ApplicationError> {
    let consumer_key: String = row
        .try_get("consumer_key")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL lag consumer 失败"))?;
    let stream_key: String = row
        .try_get("stream_key")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL lag stream 失败"))?;
    let offset: i64 = row
        .try_get("last_sequence")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL lag offset 失败"))?;
    let latest_sequence: i64 = row
        .try_get("latest_sequence")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL lag 最新序号失败"))?;
    let lag = latest_sequence
        .checked_sub(offset)
        .ok_or_else(|| storage_error("PostgreSQL realtime lag 水位倒退"))?;
    Ok(RealtimeLagSnapshot {
        stream_key,
        consumer_key,
        offset,
        latest_sequence,
        lag,
        last_advanced_at: Some(
            row.try_get("last_advanced_at")
                .map_err(|error| storage_error_from(error, "读取 PostgreSQL lag 更新时间失败"))?,
        ),
    })
}

fn validate_lag_page(
    entries: &[RealtimeLagSnapshot],
    total: i64,
    request: &RealtimeLagListRequest,
) -> Result<(), ApplicationError> {
    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("realtime lag 页大小溢出"))?;
    if total < 0 || entry_count > request.limit {
        return Err(storage_error("realtime lag 返回页越界"));
    }
    let mut previous: Option<(&str, &str)> = None;
    for entry in entries {
        if entry.offset < 0
            || entry.latest_sequence < entry.offset
            || entry.lag != entry.latest_sequence - entry.offset
            || entry.consumer_key.trim().is_empty()
            || entry.stream_key.trim().is_empty()
            || previous.is_some_and(|(consumer, stream)| {
                entry.consumer_key.as_str() < consumer
                    || (entry.consumer_key == consumer && entry.stream_key.as_str() <= stream)
            })
        {
            return Err(storage_error("realtime lag 返回投影不一致"));
        }
        previous = Some((&entry.consumer_key, &entry.stream_key));
    }
    Ok(())
}
