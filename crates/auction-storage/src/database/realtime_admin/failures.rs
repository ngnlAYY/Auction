//! Realtime failure 的分页查询、详情读取与安全投影。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::core::ApplicationError;
use auction_application::realtime::{
    RealtimeFailureListRequest, RealtimeFailurePage, RealtimeFailureSnapshot,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::postgres::PgRow;
use uuid::Uuid;

use super::common::storage_error;
#[cfg(feature = "postgres")]
pub(super) async fn list_failures_postgres(
    store: &super::super::PostgresStore,
    request: RealtimeFailureListRequest,
) -> Result<RealtimeFailurePage, ApplicationError> {
    // failure 可在读取期间被追加或解决；计数与页面必须来自同一快照。
    let mut transaction = store
        .pool()
        .begin_read_snapshot()
        .await
        .map_err(|error| storage_error_from(error, "开启 realtime failure 快照失败"))?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM public.realtime_failures AS failure
         JOIN public.realtime_events AS event ON event.event_id = failure.event_id
         WHERE event.stream_key = COALESCE($1::text, event.stream_key)
           AND failure.consumer_key = COALESCE($2::text, failure.consumer_key)
           AND failure.failure_code = COALESCE($3::text, failure.failure_code)
           AND ($4::boolean IS NULL OR (failure.resolved_at IS NOT NULL) = $4)",
    )
    .bind(request.stream_key.as_deref())
    .bind(request.consumer_key.as_deref())
    .bind(request.code.as_deref())
    .bind(request.resolved)
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime failure 总数失败"))?;
    let rows = sqlx::query(
        "SELECT failure.id, failure.event_id, event.stream_key, failure.consumer_key,
                failure.failure_code, failure.safe_detail, failure.failed_at,
                failure.resolved_at
         FROM public.realtime_failures AS failure
         JOIN public.realtime_events AS event ON event.event_id = failure.event_id
         WHERE event.stream_key = COALESCE($1::text, event.stream_key)
           AND failure.consumer_key = COALESCE($2::text, failure.consumer_key)
           AND failure.failure_code = COALESCE($3::text, failure.failure_code)
           AND ($4::boolean IS NULL OR (failure.resolved_at IS NOT NULL) = $4)
         ORDER BY failure.failed_at DESC, failure.id DESC
         LIMIT $5 OFFSET $6",
    )
    .bind(request.stream_key.as_deref())
    .bind(request.consumer_key.as_deref())
    .bind(request.code.as_deref())
    .bind(request.resolved)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime failures 失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 realtime failure 快照失败"))?;
    let entries = rows.into_iter().map(failure_from_postgres_row).collect::<Result<Vec<_>, _>>()?;
    validate_failure_page(&entries, total, &request)?;
    Ok(RealtimeFailurePage { entries, total, limit: request.limit, offset: request.offset })
}

#[cfg(feature = "postgres")]
fn failure_from_postgres_row(row: PgRow) -> Result<RealtimeFailureSnapshot, ApplicationError> {
    Ok(RealtimeFailureSnapshot {
        id: row
            .try_get("id")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL failure ID 失败"))?,
        event_id: row
            .try_get("event_id")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL failure event ID 失败"))?,
        stream_key: row
            .try_get("stream_key")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL failure stream 失败"))?,
        consumer_key: row
            .try_get("consumer_key")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL failure consumer 失败"))?,
        code: row
            .try_get("failure_code")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL failure code 失败"))?,
        safe_detail: row
            .try_get("safe_detail")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL failure detail 失败"))?,
        failed_at: row
            .try_get("failed_at")
            .map_err(|error| storage_error_from(error, "读取 PostgreSQL failure time 失败"))?,
        resolved_at: row.try_get("resolved_at").map_err(|error| {
            storage_error_from(error, "读取 PostgreSQL failure resolved time 失败")
        })?,
    })
}

fn validate_failure_page(
    entries: &[RealtimeFailureSnapshot],
    total: i64,
    request: &RealtimeFailureListRequest,
) -> Result<(), ApplicationError> {
    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("realtime failure 页大小溢出"))?;
    if total < 0 || entry_count > request.limit {
        return Err(storage_error("realtime failure 返回页越界"));
    }
    let mut previous: Option<(&DateTime<Utc>, Uuid)> = None;
    for entry in entries {
        if entry.id.is_nil()
            || entry.event_id.is_nil()
            || entry.stream_key.trim().is_empty()
            || entry.consumer_key.trim().is_empty()
            || entry.code.trim().is_empty()
            || entry.safe_detail.trim().is_empty()
            || entry.safe_detail.len() > 2_048
            || entry.resolved_at.is_some_and(|resolved| resolved < entry.failed_at)
            || previous.is_some_and(|(time, id)| {
                entry.failed_at > *time || (entry.failed_at == *time && entry.id >= id)
            })
        {
            return Err(storage_error("realtime failure 返回投影不一致"));
        }
        previous = Some((&entry.failed_at, entry.id));
    }
    Ok(())
}
