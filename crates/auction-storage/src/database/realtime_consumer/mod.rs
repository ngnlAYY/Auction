//! Worker 实时事件消费的持久化适配器。
//!
//! 游标只按 stream 的下一个连续序号推进；sink 成功但游标提交失败时允许重复投递，不能
//! 跳过事件。失败记录不改变游标，人工或运维流程只有在替代事件已经追加到同一 stream 后，
//! 才能显式越过原事件。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicRealtimeEvent, RealtimeConsumerBatch,
    RealtimeConsumerRequest, RealtimeConsumerStore, RealtimeStreamStore, RepositoryError,
};
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::Value;
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::postgres::PgRow;
#[cfg(feature = "sqlite")]
use sqlx::{Sqlite, SqliteConnection, pool::PoolConnection, sqlite::SqliteRow};
use uuid::Uuid;

use super::DatabaseStore;

impl RealtimeStreamStore for DatabaseStore {
    fn list_stream_keys(
        &self,
        prefix: String,
        after: String,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<String>> {
        Box::pin(async move {
            validate_stream_directory_query(&prefix, &after, limit)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    list_streams_postgres(store, &prefix, &after, limit).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    list_streams_sqlite(store, &prefix, &after, limit).await
                }
            }
        })
    }
}

impl RealtimeConsumerStore for DatabaseStore {
    fn read_batch(
        &self,
        request: RealtimeConsumerRequest,
    ) -> ApplicationFuture<'_, Option<RealtimeConsumerBatch>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => read_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => read_sqlite(store, request).await,
            }
        })
    }

    fn acknowledge(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        stream_sequence: i64,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            validate_cursor_keys(&consumer_key, &stream_key)?;
            validate_event_position(event_id, stream_sequence)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    acknowledge_postgres(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        stream_sequence,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    acknowledge_sqlite(store, &consumer_key, &stream_key, event_id, stream_sequence)
                        .await
                }
            }
        })
    }

    fn record_failure(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        failure_code: String,
        safe_detail: String,
        failed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            validate_cursor_keys(&consumer_key, &stream_key)?;
            validate_event_position(event_id, 1)?;
            validate_failure_text(&failure_code, &safe_detail)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    record_failure_postgres(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        &failure_code,
                        &safe_detail,
                        failed_at,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    record_failure_sqlite(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        &failure_code,
                        &safe_detail,
                        failed_at,
                    )
                    .await
                }
            }
        })
    }

    fn resolve_failure_after_replay(
        &self,
        consumer_key: String,
        stream_key: String,
        event_id: Uuid,
        replay_event_id: Uuid,
        resolved_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            validate_cursor_keys(&consumer_key, &stream_key)?;
            validate_event_position(event_id, 1)?;
            validate_event_position(replay_event_id, 1)?;
            if event_id == replay_event_id {
                return Err(storage_error("实时事件不能引用自身作为重放事件"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    resolve_failure_postgres(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        replay_event_id,
                        resolved_at,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => {
                    resolve_failure_sqlite(
                        store,
                        &consumer_key,
                        &stream_key,
                        event_id,
                        replay_event_id,
                        resolved_at,
                    )
                    .await
                }
            }
        })
    }
}

fn validate_stream_directory_query(
    prefix: &str,
    after: &str,
    limit: u16,
) -> Result<(), ApplicationError> {
    if prefix != "auction:"
        || after.len() > 255
        || after != after.trim()
        || after.chars().any(char::is_control)
        || limit == 0
        || limit > 200
    {
        return Err(storage_error("实时 stream 目录查询参数无效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn list_streams_postgres(
    store: &super::PostgresStore,
    prefix: &str,
    after: &str,
    limit: u16,
) -> Result<Vec<String>, ApplicationError> {
    let pattern = format!("{prefix}%");
    sqlx::query_scalar(
        "SELECT stream_key
         FROM public.realtime_streams
         WHERE stream_key LIKE $1 AND stream_key > $2
         ORDER BY stream_key ASC
         LIMIT $3",
    )
    .bind(pattern)
    .bind(after)
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 实时 stream 目录失败"))
}

#[cfg(feature = "sqlite")]
async fn list_streams_sqlite(
    store: &super::SqliteStore,
    prefix: &str,
    after: &str,
    limit: u16,
) -> Result<Vec<String>, ApplicationError> {
    let pattern = format!("{prefix}%");
    sqlx::query_scalar(
        "SELECT stream_key
         FROM realtime_streams
         WHERE stream_key LIKE ? AND stream_key > ?
         ORDER BY stream_key ASC
         LIMIT ?",
    )
    .bind(pattern)
    .bind(after)
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite 实时 stream 目录失败"))
}

#[cfg(feature = "postgres")]
async fn read_postgres(
    store: &super::PostgresStore,
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
async fn read_sqlite(
    store: &super::SqliteStore,
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

#[cfg(feature = "postgres")]
async fn acknowledge_postgres(
    store: &super::PostgresStore,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    stream_sequence: i64,
) -> Result<(), ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("打开 PostgreSQL 实时游标事务失败"))?;
    let result = acknowledge_postgres_inner(
        &mut transaction,
        consumer_key,
        stream_key,
        event_id,
        stream_sequence,
    )
    .await;
    match result {
        Ok(()) => transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 PostgreSQL 实时游标事务失败")),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

#[cfg(feature = "postgres")]
async fn acknowledge_postgres_inner(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    stream_sequence: i64,
) -> Result<(), ApplicationError> {
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence
         FROM public.realtime_consumer_offsets
         WHERE consumer_key = $1 AND stream_key = $2
         FOR UPDATE",
    )
    .bind(consumer_key)
    .bind(stream_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("锁定 PostgreSQL 实时消费游标失败"))?
    .unwrap_or(0);
    let stored_event_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT event_id
         FROM public.realtime_events
         WHERE stream_key = $1 AND stream_sequence = $2",
    )
    .bind(stream_key)
    .bind(stream_sequence)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("校验 PostgreSQL 实时事件序号失败"))?;
    if stored_event_id != Some(event_id) {
        return Err(storage_error("PostgreSQL 实时事件不存在或 stream 不匹配"));
    }
    if stream_sequence > current {
        let expected =
            current.checked_add(1).ok_or_else(|| storage_error("PostgreSQL 实时消费游标溢出"))?;
        if stream_sequence != expected {
            return Err(storage_error("PostgreSQL 实时消费游标不能跳过事件"));
        }
        sqlx::query(
            "INSERT INTO public.realtime_consumer_offsets
                (consumer_key, stream_key, last_sequence, updated_at)
             VALUES ($1, $2, $3, CURRENT_TIMESTAMP)
             ON CONFLICT (consumer_key, stream_key) DO UPDATE
             SET last_sequence = EXCLUDED.last_sequence, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(consumer_key)
        .bind(stream_key)
        .bind(stream_sequence)
        .execute(&mut **transaction)
        .await
        .map_err(|_| storage_error("写入 PostgreSQL 实时消费游标失败"))?;
    }
    sqlx::query(
        "UPDATE public.realtime_failures
         SET resolved_at = COALESCE(resolved_at, CURRENT_TIMESTAMP)
         WHERE consumer_key = $1 AND event_id = $2 AND replay_event_id IS NULL",
    )
    .bind(consumer_key)
    .bind(event_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("解决 PostgreSQL 实时消费失败记录失败"))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn acknowledge_sqlite(
    store: &super::SqliteStore,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    stream_sequence: i64,
) -> Result<(), ApplicationError> {
    let connection =
        store.pool().acquire().await.map_err(|_| storage_error("打开 Lite 实时游标事务失败"))?;
    let mut transaction = SqliteRealtimeTransaction::begin(connection).await?;
    let result = acknowledge_sqlite_inner(
        transaction.connection_mut()?,
        consumer_key,
        stream_key,
        event_id,
        stream_sequence,
    )
    .await;
    match result {
        Ok(()) => transaction.commit().await,
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

#[cfg(feature = "sqlite")]
async fn acknowledge_sqlite_inner(
    connection: &mut SqliteConnection,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    stream_sequence: i64,
) -> Result<(), ApplicationError> {
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence
         FROM realtime_consumer_offsets
         WHERE consumer_key = ? AND stream_key = ?",
    )
    .bind(consumer_key)
    .bind(stream_key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite 实时消费游标失败"))?
    .unwrap_or(0);
    let stored_event_id = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT event_id FROM realtime_events
         WHERE stream_key = ? AND stream_sequence = ?",
    )
    .bind(stream_key)
    .bind(stream_sequence)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| storage_error("校验 Lite 实时事件序号失败"))?
    .map(|value| uuid_from_blob(value, "Lite 实时事件标识"))
    .transpose()?;
    if stored_event_id != Some(event_id) {
        return Err(storage_error("Lite 实时事件不存在或 stream 不匹配"));
    }
    if stream_sequence > current {
        let expected =
            current.checked_add(1).ok_or_else(|| storage_error("Lite 实时消费游标溢出"))?;
        if stream_sequence != expected {
            return Err(storage_error("Lite 实时消费游标不能跳过事件"));
        }
        sqlx::query(
            "INSERT INTO realtime_consumer_offsets
                (consumer_key, stream_key, last_sequence, updated_at)
             VALUES (?, ?, ?, CURRENT_TIMESTAMP)
             ON CONFLICT(consumer_key, stream_key) DO UPDATE
             SET last_sequence = excluded.last_sequence, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(consumer_key)
        .bind(stream_key)
        .bind(stream_sequence)
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("写入 Lite 实时消费游标失败"))?;
    }
    sqlx::query(
        "UPDATE realtime_failures
         SET resolved_at = COALESCE(resolved_at, CURRENT_TIMESTAMP)
         WHERE consumer_key = ? AND event_id = ? AND replay_event_id IS NULL",
    )
    .bind(consumer_key)
    .bind(uuid_blob(event_id))
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("解决 Lite 实时消费失败记录失败"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn record_failure_postgres(
    store: &super::PostgresStore,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    failure_code: &str,
    safe_detail: &str,
    failed_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("打开 PostgreSQL 实时失败记录事务失败"))?;
    let result = async {
        let event_stream: Option<String> =
            sqlx::query_scalar("SELECT stream_key FROM public.realtime_events WHERE event_id = $1")
                .bind(event_id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(|_| storage_error("校验 PostgreSQL 实时失败事件失败"))?;
        if event_stream.as_deref() != Some(stream_key) {
            return Err(storage_error("PostgreSQL 实时失败事件不属于指定 stream"));
        }
        sqlx::query(
            "INSERT INTO public.realtime_failures
                (id, consumer_key, event_id, failure_code, safe_detail, failed_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (consumer_key, event_id) DO UPDATE
             SET failure_code = EXCLUDED.failure_code,
                 safe_detail = EXCLUDED.safe_detail,
                 failed_at = EXCLUDED.failed_at
             WHERE public.realtime_failures.resolved_at IS NULL",
        )
        .bind(Uuid::new_v4())
        .bind(consumer_key)
        .bind(event_id)
        .bind(failure_code)
        .bind(safe_detail)
        .bind(failed_at)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("写入 PostgreSQL 实时失败记录失败"))?;
        Ok::<(), ApplicationError>(())
    }
    .await;
    match result {
        Ok(()) => transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 PostgreSQL 实时失败记录事务失败")),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

#[cfg(feature = "sqlite")]
async fn record_failure_sqlite(
    store: &super::SqliteStore,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    failure_code: &str,
    safe_detail: &str,
    failed_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let connection = store
        .pool()
        .acquire()
        .await
        .map_err(|_| storage_error("打开 Lite 实时失败记录事务失败"))?;
    let mut transaction = SqliteRealtimeTransaction::begin(connection).await?;
    let result = record_failure_sqlite_inner(
        transaction.connection_mut()?,
        consumer_key,
        stream_key,
        event_id,
        failure_code,
        safe_detail,
        failed_at,
    )
    .await;
    match result {
        Ok(()) => transaction.commit().await,
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

#[cfg(feature = "sqlite")]
async fn record_failure_sqlite_inner(
    connection: &mut SqliteConnection,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    failure_code: &str,
    safe_detail: &str,
    failed_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let event_stream: Option<String> =
        sqlx::query_scalar("SELECT stream_key FROM realtime_events WHERE event_id = ?")
            .bind(uuid_blob(event_id))
            .fetch_optional(&mut *connection)
            .await
            .map_err(|_| storage_error("校验 Lite 实时失败事件失败"))?;
    if event_stream.as_deref() != Some(stream_key) {
        return Err(storage_error("Lite 实时失败事件不属于指定 stream"));
    }
    sqlx::query(
        "INSERT INTO realtime_failures
            (id, consumer_key, event_id, failure_code, safe_detail, failed_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(consumer_key, event_id) DO UPDATE
         SET failure_code = excluded.failure_code,
             safe_detail = excluded.safe_detail,
             failed_at = excluded.failed_at
         WHERE realtime_failures.resolved_at IS NULL",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(consumer_key)
    .bind(uuid_blob(event_id))
    .bind(failure_code)
    .bind(safe_detail)
    .bind(failed_at.to_rfc3339())
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("写入 Lite 实时失败记录失败"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn resolve_failure_postgres(
    store: &super::PostgresStore,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    replay_event_id: Uuid,
    resolved_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|_| storage_error("打开 PostgreSQL 实时重放事务失败"))?;
    let result = resolve_failure_postgres_inner(
        &mut transaction,
        consumer_key,
        stream_key,
        event_id,
        replay_event_id,
        resolved_at,
    )
    .await;
    match result {
        Ok(()) => transaction
            .commit()
            .await
            .map_err(|_| storage_error("提交 PostgreSQL 实时重放事务失败")),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

#[cfg(feature = "postgres")]
async fn resolve_failure_postgres_inner(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    replay_event_id: Uuid,
    resolved_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let failure = sqlx::query(
        "SELECT original.stream_key, original.stream_sequence,
                failure.resolved_at, failure.replay_event_id
         FROM public.realtime_failures AS failure
         JOIN public.realtime_events AS original ON original.event_id = failure.event_id
         WHERE failure.consumer_key = $1 AND failure.event_id = $2
         FOR UPDATE",
    )
    .bind(consumer_key)
    .bind(event_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 实时失败记录失败"))?;
    let Some(failure) = failure else {
        return Err(storage_error("PostgreSQL 实时失败记录不存在"));
    };
    let original_stream: String = failure
        .try_get("stream_key")
        .map_err(|_| storage_error("读取 PostgreSQL 原实时 stream 失败"))?;
    let original_sequence: i64 = failure
        .try_get("stream_sequence")
        .map_err(|_| storage_error("读取 PostgreSQL 原实时序号失败"))?;
    if original_stream != stream_key {
        return Err(storage_error("PostgreSQL 原实时事件不属于指定 stream"));
    }
    let existing_replay: Option<Uuid> = failure
        .try_get("replay_event_id")
        .map_err(|_| storage_error("读取 PostgreSQL 重放事件引用失败"))?;
    let already_resolved: Option<DateTime<Utc>> = failure
        .try_get("resolved_at")
        .map_err(|_| storage_error("读取 PostgreSQL 失败解决时间失败"))?;
    if let Some(existing_replay) = existing_replay {
        if existing_replay != replay_event_id {
            return Err(storage_error("PostgreSQL 实时失败记录已有其他重放事件"));
        }
        return Ok(());
    }
    if already_resolved.is_some() {
        return Err(storage_error("PostgreSQL 实时失败记录已经通过正常确认解决"));
    }
    let replay = sqlx::query(
        "SELECT stream_key, stream_sequence, replay_of_event_id
         FROM public.realtime_events WHERE event_id = $1",
    )
    .bind(replay_event_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL 重放事件失败"))?
    .ok_or_else(|| storage_error("PostgreSQL 重放事件不存在"))?;
    let replay_stream: String = replay
        .try_get("stream_key")
        .map_err(|_| storage_error("读取 PostgreSQL 重放 stream 失败"))?;
    let replay_sequence: i64 = replay
        .try_get("stream_sequence")
        .map_err(|_| storage_error("读取 PostgreSQL 重放序号失败"))?;
    let replay_of_event_id: Option<Uuid> = replay
        .try_get("replay_of_event_id")
        .map_err(|_| storage_error("读取 PostgreSQL 重放原事件引用失败"))?;
    if replay_stream != stream_key
        || replay_sequence <= original_sequence
        || replay_of_event_id != Some(event_id)
    {
        return Err(storage_error("PostgreSQL 重放事件必须位于同一 stream 的后续序号"));
    }
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence
         FROM public.realtime_consumer_offsets
         WHERE consumer_key = $1 AND stream_key = $2
         FOR UPDATE",
    )
    .bind(consumer_key)
    .bind(stream_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("锁定 PostgreSQL 重放消费游标失败"))?
    .unwrap_or(0);
    let predecessor = original_sequence
        .checked_sub(1)
        .ok_or_else(|| storage_error("PostgreSQL 原实时序号无效"))?;
    if current < predecessor {
        return Err(storage_error("PostgreSQL 实时游标尚未到达失败事件前一序号"));
    }
    if current == predecessor {
        sqlx::query(
            "INSERT INTO public.realtime_consumer_offsets
                (consumer_key, stream_key, last_sequence, updated_at)
             VALUES ($1, $2, $3, CURRENT_TIMESTAMP)
             ON CONFLICT (consumer_key, stream_key) DO UPDATE
             SET last_sequence = EXCLUDED.last_sequence, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(consumer_key)
        .bind(stream_key)
        .bind(original_sequence)
        .execute(&mut **transaction)
        .await
        .map_err(|_| storage_error("推进 PostgreSQL 重放消费游标失败"))?;
    }
    sqlx::query(
        "UPDATE public.realtime_failures
         SET resolved_at = COALESCE(resolved_at, $3), replay_event_id = $4
         WHERE consumer_key = $1 AND event_id = $2",
    )
    .bind(consumer_key)
    .bind(event_id)
    .bind(resolved_at)
    .bind(replay_event_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("解决 PostgreSQL 实时重放失败记录失败"))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn resolve_failure_sqlite(
    store: &super::SqliteStore,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    replay_event_id: Uuid,
    resolved_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let connection =
        store.pool().acquire().await.map_err(|_| storage_error("打开 Lite 实时重放事务失败"))?;
    let mut transaction = SqliteRealtimeTransaction::begin(connection).await?;
    let result = resolve_failure_sqlite_inner(
        transaction.connection_mut()?,
        consumer_key,
        stream_key,
        event_id,
        replay_event_id,
        resolved_at,
    )
    .await;
    match result {
        Ok(()) => transaction.commit().await,
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

#[cfg(feature = "sqlite")]
async fn resolve_failure_sqlite_inner(
    connection: &mut SqliteConnection,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    replay_event_id: Uuid,
    resolved_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let failure = sqlx::query(
        "SELECT original.stream_key, original.stream_sequence,
                failure.resolved_at, failure.replay_event_id
         FROM realtime_failures AS failure
         JOIN realtime_events AS original ON original.event_id = failure.event_id
         WHERE failure.consumer_key = ? AND failure.event_id = ?",
    )
    .bind(consumer_key)
    .bind(uuid_blob(event_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite 实时失败记录失败"))?;
    let Some(failure) = failure else {
        return Err(storage_error("Lite 实时失败记录不存在"));
    };
    let original_stream: String =
        failure.try_get("stream_key").map_err(|_| storage_error("读取 Lite 原实时 stream 失败"))?;
    let original_sequence: i64 = failure
        .try_get("stream_sequence")
        .map_err(|_| storage_error("读取 Lite 原实时序号失败"))?;
    if original_stream != stream_key {
        return Err(storage_error("Lite 原实时事件不属于指定 stream"));
    }
    let existing_replay = failure
        .try_get::<Option<Vec<u8>>, _>("replay_event_id")
        .map_err(|_| storage_error("读取 Lite 重放事件引用失败"))?
        .map(|value| uuid_from_blob(value, "Lite 重放事件标识"))
        .transpose()?;
    let already_resolved: Option<String> =
        failure.try_get("resolved_at").map_err(|_| storage_error("读取 Lite 失败解决时间失败"))?;
    if let Some(existing_replay) = existing_replay {
        if existing_replay != replay_event_id {
            return Err(storage_error("Lite 实时失败记录已有其他重放事件"));
        }
        return Ok(());
    }
    if already_resolved.is_some() {
        return Err(storage_error("Lite 实时失败记录已经通过正常确认解决"));
    }
    let replay = sqlx::query(
        "SELECT stream_key, stream_sequence, replay_of_event_id
             FROM realtime_events WHERE event_id = ?",
    )
    .bind(uuid_blob(replay_event_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite 重放事件失败"))?
    .ok_or_else(|| storage_error("Lite 重放事件不存在"))?;
    let replay_stream: String =
        replay.try_get("stream_key").map_err(|_| storage_error("读取 Lite 重放 stream 失败"))?;
    let replay_sequence: i64 =
        replay.try_get("stream_sequence").map_err(|_| storage_error("读取 Lite 重放序号失败"))?;
    let replay_of_event_id = replay
        .try_get::<Option<Vec<u8>>, _>("replay_of_event_id")
        .map_err(|_| storage_error("读取 Lite 重放原事件引用失败"))?
        .map(|value| uuid_from_blob(value, "Lite 重放原事件标识"))
        .transpose()?;
    if replay_stream != stream_key
        || replay_sequence <= original_sequence
        || replay_of_event_id != Some(event_id)
    {
        return Err(storage_error("Lite 重放事件必须位于同一 stream 的后续序号"));
    }
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence FROM realtime_consumer_offsets
         WHERE consumer_key = ? AND stream_key = ?",
    )
    .bind(consumer_key)
    .bind(stream_key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| storage_error("读取 Lite 重放消费游标失败"))?
    .unwrap_or(0);
    let predecessor =
        original_sequence.checked_sub(1).ok_or_else(|| storage_error("Lite 原实时序号无效"))?;
    if current < predecessor {
        return Err(storage_error("Lite 实时游标尚未到达失败事件前一序号"));
    }
    if current == predecessor {
        sqlx::query(
            "INSERT INTO realtime_consumer_offsets
                (consumer_key, stream_key, last_sequence, updated_at)
             VALUES (?, ?, ?, CURRENT_TIMESTAMP)
             ON CONFLICT(consumer_key, stream_key) DO UPDATE
             SET last_sequence = excluded.last_sequence, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(consumer_key)
        .bind(stream_key)
        .bind(original_sequence)
        .execute(&mut *connection)
        .await
        .map_err(|_| storage_error("推进 Lite 重放消费游标失败"))?;
    }
    sqlx::query(
        "UPDATE realtime_failures
         SET resolved_at = COALESCE(resolved_at, ?), replay_event_id = ?
         WHERE consumer_key = ? AND event_id = ?",
    )
    .bind(resolved_at.to_rfc3339())
    .bind(uuid_blob(replay_event_id))
    .bind(consumer_key)
    .bind(uuid_blob(event_id))
    .execute(&mut *connection)
    .await
    .map_err(|_| storage_error("解决 Lite 实时重放失败记录失败"))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
struct SqliteRealtimeTransaction {
    connection: Option<PoolConnection<Sqlite>>,
}

#[cfg(feature = "sqlite")]
impl SqliteRealtimeTransaction {
    async fn begin(connection: PoolConnection<Sqlite>) -> Result<Self, ApplicationError> {
        let mut transaction = Self { connection: Some(connection) };
        sqlx::query("BEGIN IMMEDIATE")
            .execute(transaction.connection_mut()?)
            .await
            .map_err(|_| storage_error("开始 Lite 实时写事务失败"))?;
        Ok(transaction)
    }

    async fn commit(mut self) -> Result<(), ApplicationError> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        let result = sqlx::query("COMMIT").execute(&mut *connection).await;
        if result.is_err() {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        }
        result.map(|_| ()).map_err(|_| storage_error("提交 Lite 实时写事务失败"))
    }

    async fn rollback(mut self) -> Result<(), ApplicationError> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .map(|_| ())
            .map_err(|_| storage_error("回滚 Lite 实时写事务失败"))
    }

    fn connection_mut(&mut self) -> Result<&mut SqliteConnection, ApplicationError> {
        self.connection.as_deref_mut().ok_or_else(|| storage_error("Lite 实时写事务已经结束"))
    }
}

fn validate_cursor_keys(consumer_key: &str, stream_key: &str) -> Result<(), ApplicationError> {
    RealtimeConsumerRequest {
        consumer_key: consumer_key.to_owned(),
        stream_key: stream_key.to_owned(),
        batch_size: 1,
        allow_discovery_stream: true,
    }
    .validate()
    .map_err(storage_error)
}

fn validate_event_position(event_id: Uuid, stream_sequence: i64) -> Result<(), ApplicationError> {
    if event_id.is_nil() || stream_sequence <= 0 {
        return Err(storage_error("实时事件标识或序号无效"));
    }
    Ok(())
}

fn validate_failure_text(failure_code: &str, safe_detail: &str) -> Result<(), ApplicationError> {
    if failure_code.trim().is_empty()
        || failure_code != failure_code.trim()
        || failure_code.len() > 128
        || failure_code.chars().any(char::is_control)
        || safe_detail.trim().is_empty()
        || safe_detail != safe_detail.trim()
        || safe_detail.len() > 2_048
        || safe_detail.chars().any(char::is_control)
    {
        return Err(storage_error("实时消费失败分类或安全说明无效"));
    }
    Ok(())
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

#[cfg(feature = "sqlite")]
fn uuid_blob(value: Uuid) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
fn parse_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = chrono::NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error("Lite 实时事件时间格式无效"))
}
