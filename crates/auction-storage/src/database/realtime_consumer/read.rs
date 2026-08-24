//! Realtime 事件批量读取与公共事件安全映射。

use auction_application::{
    ApplicationError, PublicRealtimeEvent, RealtimeConsumerBatch, RealtimeConsumerRequest,
};
#[cfg(feature = "postgres")]
use serde_json::Value;
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::postgres::PgRow;
#[cfg(feature = "sqlite")]
use sqlx::sqlite::SqliteRow;

use super::common::storage_error;
#[cfg(feature = "sqlite")]
use super::common::{parse_timestamp, uuid_from_blob};
#[cfg(feature = "postgres")]
pub(super) async fn read_postgres(
    store: &super::super::PostgresStore,
    request: RealtimeConsumerRequest,
) -> Result<Option<RealtimeConsumerBatch>, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("打开 PostgreSQL 实时消费快照失败"))?;
    let stream_exists: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM public.realtime_streams WHERE stream_key = $1")
            .bind(&request.stream_key)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|_| storage_error("读取 PostgreSQL 实时 stream 目录失败"))?;
    if stream_exists.is_none() {
        transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 PostgreSQL 实时 stream 快照失败"))?;
        return Ok(None);
    }

    let after = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence
         FROM public.realtime_consumer_offsets
         WHERE consumer_key = $1 AND stream_key = $2",
    )
    .bind(&request.consumer_key)
    .bind(&request.stream_key)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 实时消费游标失败"))?
    .unwrap_or(0);
    let rows = sqlx::query(
        "SELECT event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, created_at
         FROM public.realtime_events
         WHERE stream_key = $1 AND stream_sequence > $2
         ORDER BY stream_sequence ASC
         LIMIT $3",
    )
    .bind(&request.stream_key)
    .bind(after)
    .bind(i64::from(request.batch_size))
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 实时消费事件失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交 PostgreSQL 实时消费快照失败"))?;

    let events = rows.into_iter().map(event_from_postgres_row).collect::<Result<Vec<_>, _>>()?;
    Ok(Some(batch(request, after, events)))
}

#[cfg(feature = "sqlite")]
pub(super) async fn read_sqlite(
    store: &super::super::SqliteStore,
    request: RealtimeConsumerRequest,
) -> Result<Option<RealtimeConsumerBatch>, ApplicationError> {
    let mut connection =
        store.pool().acquire().await.map_err(|_| storage_error("打开 Lite 实时消费快照失败"))?;
    let stream_exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM realtime_streams WHERE stream_key = ?")
            .bind(&request.stream_key)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|_| storage_error("读取 Lite 实时 stream 目录失败"))?;
    if stream_exists.is_none() {
        return Ok(None);
    }

    let after = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence
         FROM realtime_consumer_offsets
         WHERE consumer_key = ? AND stream_key = ?",
    )
    .bind(&request.consumer_key)
    .bind(&request.stream_key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite 实时消费游标失败"))?
    .unwrap_or(0);
    let rows = sqlx::query(
        "SELECT event_id, stream_key, stream_sequence, kind, schema_version,
                public_payload, created_at
         FROM realtime_events
         WHERE stream_key = ? AND stream_sequence > ?
         ORDER BY stream_sequence ASC
         LIMIT ?",
    )
    .bind(&request.stream_key)
    .bind(after)
    .bind(i64::from(request.batch_size))
    .fetch_all(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite 实时消费事件失败"))?;

    let events = rows.into_iter().map(event_from_sqlite_row).collect::<Result<Vec<_>, _>>()?;
    Ok(Some(batch(request, after, events)))
}

fn batch(
    request: RealtimeConsumerRequest,
    after: i64,
    events: Vec<PublicRealtimeEvent>,
) -> RealtimeConsumerBatch {
    let sequence_gap = events.first().is_some_and(|event| {
        event.stream_sequence > after.saturating_add(1)
            || event.stream_sequence <= after
            || event.stream_key != request.stream_key
    });
    RealtimeConsumerBatch {
        consumer_key: request.consumer_key,
        stream_key: request.stream_key,
        after,
        events,
        sequence_gap,
    }
}

#[cfg(feature = "postgres")]
fn event_from_postgres_row(row: PgRow) -> Result<PublicRealtimeEvent, ApplicationError> {
    Ok(PublicRealtimeEvent {
        event_id: row
            .try_get("event_id")
            .map_err(|_| storage_error("读取 PostgreSQL 实时事件标识失败"))?,
        stream_key: row
            .try_get("stream_key")
            .map_err(|_| storage_error("读取 PostgreSQL 实时事件流名失败"))?,
        stream_sequence: row
            .try_get("stream_sequence")
            .map_err(|_| storage_error("读取 PostgreSQL 实时事件序号失败"))?,
        kind: row.try_get("kind").map_err(|_| storage_error("读取 PostgreSQL 实时事件类型失败"))?,
        schema_version: row
            .try_get("schema_version")
            .map_err(|_| storage_error("读取 PostgreSQL 实时事件版本失败"))?,
        payload: row
            .try_get::<sqlx::types::Json<Value>, _>("public_payload")
            .map_err(|_| storage_error("读取 PostgreSQL 实时事件负载失败"))?
            .0,
        created_at: row
            .try_get("created_at")
            .map_err(|_| storage_error("读取 PostgreSQL 实时事件时间失败"))?,
    })
}

#[cfg(feature = "sqlite")]
fn event_from_sqlite_row(row: SqliteRow) -> Result<PublicRealtimeEvent, ApplicationError> {
    Ok(PublicRealtimeEvent {
        event_id: uuid_from_blob(
            row.try_get("event_id").map_err(|_| storage_error("读取 Lite 实时事件标识失败"))?,
            "Lite 实时事件标识",
        )?,
        stream_key: row
            .try_get("stream_key")
            .map_err(|_| storage_error("读取 Lite 实时事件流名失败"))?,
        stream_sequence: row
            .try_get("stream_sequence")
            .map_err(|_| storage_error("读取 Lite 实时事件序号失败"))?,
        kind: row.try_get("kind").map_err(|_| storage_error("读取 Lite 实时事件类型失败"))?,
        schema_version: row
            .try_get("schema_version")
            .map_err(|_| storage_error("读取 Lite 实时事件版本失败"))?,
        payload: serde_json::from_str(
            &row.try_get::<String, _>("public_payload")
                .map_err(|_| storage_error("读取 Lite 实时事件负载失败"))?,
        )
        .map_err(|_| storage_error("Lite 实时事件负载不是有效 JSON"))?,
        created_at: parse_timestamp(
            row.try_get("created_at").map_err(|_| storage_error("读取 Lite 实时事件时间失败"))?,
        )?,
    })
}
