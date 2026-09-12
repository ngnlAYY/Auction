//! Realtime consumer failure 记录事务。

use auction_application::ApplicationError;
use chrono::{DateTime, Utc};
#[cfg(feature = "sqlite")]
use sqlx::SqliteConnection;
use uuid::Uuid;

#[cfg(feature = "sqlite")]
use super::super::super::SqliteStore;
use super::super::common::storage_error;
#[cfg(feature = "sqlite")]
use super::super::common::{SqliteRealtimeTransaction, uuid_blob};
#[cfg(feature = "postgres")]
use sqlx::Connection;

#[cfg(feature = "postgres")]
pub(crate) async fn record_failure_postgres(
    connection: &mut sqlx::PgConnection,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    failure_code: &str,
    safe_detail: &str,
    failed_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let mut transaction = connection
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
pub(crate) async fn record_failure_sqlite(
    store: &SqliteStore,
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
