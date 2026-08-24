//! Realtime consumer failure 的记录与 replay 后收口事务。

use auction_application::ApplicationError;
use chrono::{DateTime, Utc};
use sqlx::Row;
#[cfg(feature = "sqlite")]
use sqlx::SqliteConnection;
use uuid::Uuid;

use super::common::storage_error;
#[cfg(feature = "sqlite")]
use super::common::{SqliteRealtimeTransaction, uuid_blob, uuid_from_blob};
#[cfg(feature = "postgres")]
pub(super) async fn record_failure_postgres(
    store: &super::super::PostgresStore,
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
pub(super) async fn record_failure_sqlite(
    store: &super::super::SqliteStore,
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
pub(super) async fn resolve_failure_postgres(
    store: &super::super::PostgresStore,
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
pub(super) async fn resolve_failure_sqlite(
    store: &super::super::SqliteStore,
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
