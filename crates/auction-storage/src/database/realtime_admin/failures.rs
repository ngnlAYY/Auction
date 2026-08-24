//! Realtime failure 的分页查询、详情读取与安全投影。

use auction_application::{
    ApplicationError, RealtimeFailureListRequest, RealtimeFailurePage, RealtimeFailureSnapshot,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::postgres::PgRow;
#[cfg(feature = "sqlite")]
use sqlx::sqlite::SqliteRow;
use uuid::Uuid;

use super::common::storage_error;
#[cfg(feature = "sqlite")]
use super::common::{parse_timestamp, uuid_blob, uuid_from_blob};
#[cfg(feature = "postgres")]
pub(super) async fn list_failures_postgres(
    store: &super::super::PostgresStore,
    request: RealtimeFailureListRequest,
) -> Result<RealtimeFailurePage, ApplicationError> {
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
    .fetch_one(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL realtime failure 总数失败"))?;
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
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL realtime failures 失败"))?;
    let entries = rows.into_iter().map(failure_from_postgres_row).collect::<Result<Vec<_>, _>>()?;
    validate_failure_page(&entries, total, &request)?;
    Ok(RealtimeFailurePage { entries, total, limit: request.limit, offset: request.offset })
}

#[cfg(feature = "sqlite")]
pub(super) async fn list_failures_sqlite(
    store: &super::super::SqliteStore,
    request: RealtimeFailureListRequest,
) -> Result<RealtimeFailurePage, ApplicationError> {
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM realtime_failures AS failure
         JOIN realtime_events AS event ON event.event_id = failure.event_id
         WHERE event.stream_key = COALESCE(?, event.stream_key)
           AND failure.consumer_key = COALESCE(?, failure.consumer_key)
           AND failure.failure_code = COALESCE(?, failure.failure_code)
           AND COALESCE(?, (failure.resolved_at IS NOT NULL)) = (failure.resolved_at IS NOT NULL)",
    )
    .bind(request.stream_key.as_deref())
    .bind(request.consumer_key.as_deref())
    .bind(request.code.as_deref())
    .bind(request.resolved)
    .fetch_one(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite realtime failure 总数失败"))?;
    let rows = sqlx::query(
        "SELECT failure.id, failure.event_id, event.stream_key, failure.consumer_key,
                failure.failure_code, failure.safe_detail, failure.failed_at,
                failure.resolved_at
         FROM realtime_failures AS failure
         JOIN realtime_events AS event ON event.event_id = failure.event_id
         WHERE event.stream_key = COALESCE(?, event.stream_key)
           AND failure.consumer_key = COALESCE(?, failure.consumer_key)
           AND failure.failure_code = COALESCE(?, failure.failure_code)
           AND COALESCE(?, (failure.resolved_at IS NOT NULL)) = (failure.resolved_at IS NOT NULL)
         ORDER BY failure.failed_at DESC, failure.id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(request.stream_key.as_deref())
    .bind(request.consumer_key.as_deref())
    .bind(request.code.as_deref())
    .bind(request.resolved)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite realtime failures 失败"))?;
    let entries = rows.into_iter().map(failure_from_sqlite_row).collect::<Result<Vec<_>, _>>()?;
    validate_failure_page(&entries, total, &request)?;
    Ok(RealtimeFailurePage { entries, total, limit: request.limit, offset: request.offset })
}

#[cfg(feature = "postgres")]
fn failure_from_postgres_row(row: PgRow) -> Result<RealtimeFailureSnapshot, ApplicationError> {
    Ok(RealtimeFailureSnapshot {
        id: row.try_get("id").map_err(|_| storage_error("读取 PostgreSQL failure ID 失败"))?,
        event_id: row
            .try_get("event_id")
            .map_err(|_| storage_error("读取 PostgreSQL failure event ID 失败"))?,
        stream_key: row
            .try_get("stream_key")
            .map_err(|_| storage_error("读取 PostgreSQL failure stream 失败"))?,
        consumer_key: row
            .try_get("consumer_key")
            .map_err(|_| storage_error("读取 PostgreSQL failure consumer 失败"))?,
        code: row
            .try_get("failure_code")
            .map_err(|_| storage_error("读取 PostgreSQL failure code 失败"))?,
        safe_detail: row
            .try_get("safe_detail")
            .map_err(|_| storage_error("读取 PostgreSQL failure detail 失败"))?,
        failed_at: row
            .try_get("failed_at")
            .map_err(|_| storage_error("读取 PostgreSQL failure time 失败"))?,
        resolved_at: row
            .try_get("resolved_at")
            .map_err(|_| storage_error("读取 PostgreSQL failure resolved time 失败"))?,
    })
}

#[cfg(feature = "sqlite")]
fn failure_from_sqlite_row(row: SqliteRow) -> Result<RealtimeFailureSnapshot, ApplicationError> {
    Ok(RealtimeFailureSnapshot {
        id: uuid_from_blob(
            row.try_get("id").map_err(|_| storage_error("读取 Lite failure ID 失败"))?,
            "Lite failure ID",
        )?,
        event_id: uuid_from_blob(
            row.try_get("event_id")
                .map_err(|_| storage_error("读取 Lite failure event ID 失败"))?,
            "Lite failure event ID",
        )?,
        stream_key: row
            .try_get("stream_key")
            .map_err(|_| storage_error("读取 Lite failure stream 失败"))?,
        consumer_key: row
            .try_get("consumer_key")
            .map_err(|_| storage_error("读取 Lite failure consumer 失败"))?,
        code: row
            .try_get("failure_code")
            .map_err(|_| storage_error("读取 Lite failure code 失败"))?,
        safe_detail: row
            .try_get("safe_detail")
            .map_err(|_| storage_error("读取 Lite failure detail 失败"))?,
        failed_at: parse_timestamp(
            row.try_get("failed_at").map_err(|_| storage_error("读取 Lite failure time 失败"))?,
        )?,
        resolved_at: row
            .try_get::<Option<String>, _>("resolved_at")
            .map_err(|_| storage_error("读取 Lite failure resolved time 失败"))?
            .map(parse_timestamp)
            .transpose()?,
    })
}

fn validate_failure_page(
    entries: &[RealtimeFailureSnapshot],
    total: i64,
    request: &RealtimeFailureListRequest,
) -> Result<(), ApplicationError> {
    if total < 0 || entries.len() as i64 > request.limit {
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

#[cfg(feature = "postgres")]
pub(super) async fn get_failure_postgres(
    store: &super::super::PostgresStore,
    failure_id: Uuid,
) -> Result<Option<RealtimeFailureSnapshot>, ApplicationError> {
    let row = sqlx::query(
        "SELECT failure.id, failure.event_id, event.stream_key, failure.consumer_key,
                failure.failure_code, failure.safe_detail, failure.failed_at,
                failure.resolved_at
         FROM public.realtime_failures AS failure
         JOIN public.realtime_events AS event ON event.event_id = failure.event_id
         WHERE failure.id = $1",
    )
    .bind(failure_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL realtime failure 详情失败"))?;
    row.map(failure_from_postgres_row).transpose()
}

#[cfg(feature = "sqlite")]
pub(super) async fn get_failure_sqlite(
    store: &super::super::SqliteStore,
    failure_id: Uuid,
) -> Result<Option<RealtimeFailureSnapshot>, ApplicationError> {
    let row = sqlx::query(
        "SELECT failure.id, failure.event_id, event.stream_key, failure.consumer_key,
                failure.failure_code, failure.safe_detail, failure.failed_at,
                failure.resolved_at
         FROM realtime_failures AS failure
         JOIN realtime_events AS event ON event.event_id = failure.event_id
         WHERE failure.id = ?",
    )
    .bind(uuid_blob(failure_id))
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite realtime failure 详情失败"))?;
    row.map(failure_from_sqlite_row).transpose()
}
