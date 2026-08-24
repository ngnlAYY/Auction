//! Realtime consumer 共享校验、错误映射与 SQLite 串行写事务。

use auction_application::{ApplicationError, RealtimeConsumerRequest, RepositoryError};
#[cfg(feature = "sqlite")]
use chrono::{DateTime, Utc};
#[cfg(feature = "sqlite")]
use sqlx::{Sqlite, SqliteConnection, pool::PoolConnection};
use uuid::Uuid;

#[cfg(feature = "sqlite")]
pub(super) struct SqliteRealtimeTransaction {
    connection: Option<PoolConnection<Sqlite>>,
}

#[cfg(feature = "sqlite")]
impl SqliteRealtimeTransaction {
    pub(super) async fn begin(
        connection: PoolConnection<Sqlite>,
    ) -> Result<Self, ApplicationError> {
        let mut transaction = Self { connection: Some(connection) };
        sqlx::query("BEGIN IMMEDIATE")
            .execute(transaction.connection_mut()?)
            .await
            .map_err(|_| storage_error("开始 Lite 实时写事务失败"))?;
        Ok(transaction)
    }

    pub(super) async fn commit(mut self) -> Result<(), ApplicationError> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        let result = sqlx::query("COMMIT").execute(&mut *connection).await;
        if result.is_err() {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
        }
        result.map(|_| ()).map_err(|_| storage_error("提交 Lite 实时写事务失败"))
    }

    pub(super) async fn rollback(mut self) -> Result<(), ApplicationError> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .map(|_| ())
            .map_err(|_| storage_error("回滚 Lite 实时写事务失败"))
    }

    pub(super) fn connection_mut(&mut self) -> Result<&mut SqliteConnection, ApplicationError> {
        self.connection.as_deref_mut().ok_or_else(|| storage_error("Lite 实时写事务已经结束"))
    }
}

pub(super) fn validate_cursor_keys(
    consumer_key: &str,
    stream_key: &str,
) -> Result<(), ApplicationError> {
    RealtimeConsumerRequest {
        consumer_key: consumer_key.to_owned(),
        stream_key: stream_key.to_owned(),
        batch_size: 1,
        allow_discovery_stream: true,
    }
    .validate()
    .map_err(storage_error)
}

pub(super) fn validate_event_position(
    event_id: Uuid,
    stream_sequence: i64,
) -> Result<(), ApplicationError> {
    if event_id.is_nil() || stream_sequence <= 0 {
        return Err(storage_error("实时事件标识或序号无效"));
    }
    Ok(())
}

pub(super) fn validate_failure_text(
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), ApplicationError> {
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

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

#[cfg(feature = "sqlite")]
pub(super) fn uuid_blob(value: Uuid) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
pub(super) fn uuid_from_blob(bytes: Vec<u8>, label: &str) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error(label))
}

#[cfg(feature = "sqlite")]
pub(super) fn parse_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
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
