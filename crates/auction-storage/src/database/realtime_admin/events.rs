//! Realtime 事件元数据的安全分页查询。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::core::ApplicationError;
use auction_application::realtime::{
    RealtimeEventListRequest, RealtimeEventPage, RealtimeEventSnapshot,
};
use chrono::{DateTime, Utc};
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::postgres::PgRow;
use uuid::Uuid;

use super::common::storage_error;

#[cfg(feature = "postgres")]
pub(super) async fn list_events_postgres(
    store: &super::super::DatabaseStore,
    request: RealtimeEventListRequest,
) -> Result<RealtimeEventPage, ApplicationError> {
    // payload 与内部来源字段不进入 SELECT，从 SQL 投影上保证 Admin 端无法误返回它们。
    // 页与计数分别规划，避免双重引用让全量元数据物化后再排序；两者仍属于同一语句快照。
    let rows = sqlx::query(
        "WITH filtered AS NOT MATERIALIZED (
             SELECT event_id, stream_key, stream_sequence, kind, schema_version, created_at
             FROM public.realtime_events
             WHERE stream_key = COALESCE($1::text, stream_key)
               AND kind = COALESCE($2::text, kind)
         ), page AS (
             SELECT event_id, stream_key, stream_sequence, kind, schema_version, created_at
             FROM filtered
             ORDER BY created_at DESC, event_id DESC
             LIMIT $3 OFFSET $4
         ), summary AS (
             SELECT COUNT(*)::bigint AS total FROM filtered
         )
         SELECT page.event_id, page.stream_key, page.stream_sequence, page.kind,
                page.schema_version, page.created_at, summary.total
         FROM summary
         LEFT JOIN page ON TRUE
         ORDER BY page.created_at DESC NULLS LAST, page.event_id DESC NULLS LAST",
    )
    .bind(request.stream_key.as_deref())
    .bind(request.kind.as_deref())
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime 事件元数据失败"))?;
    let total = rows
        .first()
        .ok_or_else(|| storage_error("PostgreSQL realtime 事件摘要缺失"))?
        .try_get("total")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime 事件总数失败"))?;
    let entries = rows
        .into_iter()
        .filter_map(|row| event_from_postgres_page_row(row).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    validate_event_page(&entries, total, &request)?;
    Ok(RealtimeEventPage { entries, total, limit: request.limit, offset: request.offset })
}

#[cfg(feature = "postgres")]
fn event_from_postgres_page_row(
    row: PgRow,
) -> Result<Option<RealtimeEventSnapshot>, ApplicationError> {
    let event_id: Option<Uuid> = row
        .try_get("event_id")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime 事件 ID 失败"))?;
    let stream_key: Option<String> = row
        .try_get("stream_key")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime 事件流失败"))?;
    let stream_sequence: Option<i64> = row
        .try_get("stream_sequence")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime 事件序号失败"))?;
    let kind: Option<String> = row
        .try_get("kind")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime 事件类型失败"))?;
    let schema_version: Option<i32> = row
        .try_get("schema_version")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime schema 版本失败"))?;
    let created_at: Option<DateTime<Utc>> = row
        .try_get("created_at")
        .map_err(|error| storage_error_from(error, "读取 PostgreSQL realtime 事件时间失败"))?;
    match (event_id, stream_key, stream_sequence, kind, schema_version, created_at) {
        (None, None, None, None, None, None) => Ok(None),
        (
            Some(event_id),
            Some(stream_key),
            Some(stream_sequence),
            Some(kind),
            Some(schema_version),
            Some(created_at),
        ) => Ok(Some(RealtimeEventSnapshot {
            event_id,
            stream_key,
            stream_sequence,
            kind,
            schema_version,
            created_at,
        })),
        _ => Err(storage_error("PostgreSQL realtime 事件分页行不完整")),
    }
}

fn validate_event_page(
    entries: &[RealtimeEventSnapshot],
    total: i64,
    request: &RealtimeEventListRequest,
) -> Result<(), ApplicationError> {
    let entry_count =
        i64::try_from(entries.len()).map_err(|_| storage_error("realtime 事件页大小溢出"))?;
    let expected_count = total.saturating_sub(request.offset).clamp(0, request.limit);
    if total < 0 || entry_count != expected_count {
        return Err(storage_error("realtime 事件返回页越界"));
    }
    let mut previous: Option<(&DateTime<Utc>, Uuid)> = None;
    for entry in entries {
        if entry.event_id.is_nil()
            || !auction_domain::valid_text(&entry.stream_key, 256)
            || entry.stream_sequence <= 0
            || !auction_domain::valid_text(&entry.kind, 128)
            || entry.schema_version <= 0
            || entry.created_at.timestamp_nanos_opt().is_none()
            || request
                .stream_key
                .as_deref()
                .is_some_and(|stream_key| entry.stream_key != stream_key)
            || request.kind.as_deref().is_some_and(|kind| entry.kind != kind)
            || previous.is_some_and(|(created_at, event_id)| {
                entry.created_at > *created_at
                    || (entry.created_at == *created_at && entry.event_id >= event_id)
            })
        {
            return Err(storage_error("realtime 事件返回投影不一致"));
        }
        previous = Some((&entry.created_at, entry.event_id));
    }
    Ok(())
}
