//! 会话适配器共享错误与 SQLite 映射。

use auction_application::{ApplicationError, RepositoryError};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
#[cfg(feature = "sqlite")]
use chrono::{DateTime, Utc};
#[cfg(feature = "sqlite")]
use uuid::Uuid;

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

#[cfg(feature = "sqlite")]
pub(super) fn uuid_blob(id: Uuid) -> Vec<u8> {
    id.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
pub(super) fn uuid_from_blob(bytes: Vec<u8>) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&bytes).map_err(|_| storage_error("Lite 会话用户标识格式无效"))
}

#[cfg(feature = "sqlite")]
pub(super) fn sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

#[cfg(feature = "sqlite")]
pub(super) fn parse_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
        return Ok(value.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(value, Utc));
        }
    }
    Err(storage_error("Lite 会话时间格式无效"))
}
