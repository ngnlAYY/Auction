//! Realtime 事件元数据的安全分页查询。

use auction_application::core::ApplicationError;
use auction_application::realtime::{
    RealtimeEventListRequest, RealtimeEventPage, RealtimeEventSnapshot,
};
use auction_time::Timestamp;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use crate::database::storage_error;
use crate::database::storage_error_from;

// payload 与内部来源字段不进入 SELECT，从 SQL 投影上保证 Admin 端无法误返回它们。
// 页与计数分别规划，避免双重引用让全量元数据物化后再排序；两者仍属于同一语句快照。
// 竞价事件提交后才补号；列表和总数只反映当前快照中已进入事件流的记录。
const LIST_EVENTS_SQL: &str = "WITH filtered AS NOT MATERIALIZED (
             SELECT event_id, stream_key, stream_sequence, kind, schema_version, created_at
             FROM public.realtime_events
             WHERE stream_sequence IS NOT NULL
               AND stream_key = COALESCE($1::text, stream_key)
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
         ORDER BY page.created_at DESC NULLS LAST, page.event_id DESC NULLS LAST";

pub(super) async fn list_events_postgres(
    store: &crate::database::DatabaseStore,
    request: RealtimeEventListRequest,
) -> Result<RealtimeEventPage, ApplicationError> {
    let rows = sqlx::query(LIST_EVENTS_SQL)
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
    let created_at: Option<Timestamp> = row
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
    let mut previous: Option<(&Timestamp, Uuid)> = None;
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

#[cfg(test)]
mod tests {
    use sqlx::{AssertSqlSafe, Connection, Executor, Row};
    use uuid::Uuid;

    use super::{LIST_EVENTS_SQL, event_from_postgres_page_row};

    #[tokio::test]
    #[ignore = "需要 AUCTION_TEST_DATABASE_URL 指向隔离的 PostgreSQL 18"]
    async fn postgres_admin_events_page_and_total_only_include_assigned_sequences()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = std::env::var("AUCTION_TEST_DATABASE_URL")?;
        let mut connection = sqlx::PgConnection::connect(&url).await?;
        let mut transaction = connection.begin().await?;
        transaction
            .execute(
                "CREATE TEMP TABLE realtime_events (
                    event_id uuid, stream_key text, stream_sequence bigint, kind text,
                    schema_version integer, created_at timestamptz
                );
                INSERT INTO pg_temp.realtime_events VALUES
                  ('00000000-0000-0000-0000-000000000001', 'auction:test', 1, 'lot.bid_accepted',
                   2, '2026-11-06T10:00:00Z'),
                  ('00000000-0000-0000-0000-000000000002', 'auction:test', NULL, 'lot.bid_accepted',
                   2, '2026-11-06T10:00:01Z');",
            )
            .await?;
        // 复用生产查询，只将表名指向连接私有夹具。
        let sql = LIST_EVENTS_SQL.replace("public.realtime_events", "pg_temp.realtime_events");
        for offset in [0_i64, 1] {
            let row = sqlx::query(AssertSqlSafe(sql.as_str()))
                .bind("auction:test")
                .bind("lot.bid_accepted")
                .bind(1_i64)
                .bind(offset)
                .fetch_one(&mut *transaction)
                .await?;
            assert_eq!(row.try_get::<i64, _>("total")?, 1);
            let entry = event_from_postgres_page_row(row)?;
            assert_eq!(
                entry.as_ref().map(|entry| entry.event_id),
                (offset == 0).then_some(Uuid::from_u128(1))
            );
            assert_eq!(entry.map(|entry| entry.stream_sequence), (offset == 0).then_some(1));
        }
        transaction.rollback().await?;
        Ok(())
    }
}
