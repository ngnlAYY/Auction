//! Realtime Admin 适配器的共享错误、UUID/时间映射与 Lite 事务边界。

use auction_application::{ApplicationError, RealtimeAdminError, RepositoryError};
#[cfg(feature = "sqlite")]
use chrono::{DateTime, Utc};
#[cfg(feature = "sqlite")]
use sqlx::{Sqlite, SqliteConnection, pool::PoolConnection};
#[cfg(feature = "sqlite")]
use uuid::Uuid;

#[cfg(feature = "sqlite")]
pub(super) struct SqliteRealtimeAdminTransaction {
    connection: Option<PoolConnection<Sqlite>>,
}

#[cfg(feature = "sqlite")]
impl SqliteRealtimeAdminTransaction {
    pub(super) async fn begin(
        connection: PoolConnection<Sqlite>,
    ) -> Result<Self, ApplicationError> {
        let mut transaction = Self { connection: Some(connection) };
        sqlx::query("BEGIN IMMEDIATE")
            .execute(transaction.connection_mut()?)
            .await
            .map_err(|_| storage_error("开始 Lite realtime replay 写事务失败"))?;
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
        result.map(|_| ()).map_err(|_| storage_error("提交 Lite realtime replay 写事务失败"))
    }

    pub(super) async fn rollback(mut self) -> Result<(), ApplicationError> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        sqlx::query("ROLLBACK")
            .execute(&mut *connection)
            .await
            .map(|_| ())
            .map_err(|_| storage_error("回滚 Lite realtime replay 写事务失败"))
    }

    pub(super) fn connection_mut(&mut self) -> Result<&mut SqliteConnection, ApplicationError> {
        self.connection
            .as_deref_mut()
            .ok_or_else(|| storage_error("Lite realtime replay 事务已经结束"))
    }
}

pub(super) fn admin_error(error: RealtimeAdminError) -> ApplicationError {
    ApplicationError::RealtimeAdmin(error)
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
pub(super) fn optional_uuid_from_blob(
    bytes: Option<Vec<u8>>,
    label: &str,
) -> Result<Option<Uuid>, ApplicationError> {
    bytes.map(|value| uuid_from_blob(value, label)).transpose()
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
    Err(storage_error("Lite realtime 时间格式无效"))
}
