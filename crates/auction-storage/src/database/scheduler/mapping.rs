//! Worker 调度共享阶段、命令、UUID、时间与错误映射。

use auction_application::ScheduledRoundPhase;
use auction_domain::{CommandId, deterministic_uuid};
#[cfg(feature = "sqlite")]
use chrono::DateTime;
#[cfg(feature = "sqlite")]
use chrono::Utc;
use uuid::Uuid;

pub(crate) fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn command_id(create_command_id: Uuid, phase: ScheduledRoundPhase) -> CommandId {
    let label = match phase {
        ScheduledRoundPhase::Preview => "scheduled-preview",
        ScheduledRoundPhase::Open => "scheduled-open",
        ScheduledRoundPhase::Final => "scheduled-final",
    };
    CommandId::from_uuid(deterministic_uuid(CommandId::from_uuid(create_command_id), label, 1))
}

pub(crate) fn phase_from_completion(
    preview_completed: bool,
    open_completed: bool,
) -> ScheduledRoundPhase {
    if !preview_completed {
        ScheduledRoundPhase::Preview
    } else if !open_completed {
        ScheduledRoundPhase::Open
    } else {
        ScheduledRoundPhase::Final
    }
}

pub(crate) fn repository_error(message: &str) -> auction_application::ApplicationError {
    auction_application::RepositoryError::Failed(message.to_owned()).into()
}

pub(crate) fn repository_error_owned(message: &str) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(message.to_owned())
}

#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) fn map_storage_error(error: sqlx::Error) -> auction_application::RepositoryError {
    auction_application::RepositoryError::Failed(match error {
        sqlx::Error::RowNotFound => "Worker 调度记录不存在".to_owned(),
        _ => "Worker 调度数据库操作失败".to_owned(),
    })
}

#[cfg(feature = "sqlite")]
pub(crate) fn sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string()
}

#[cfg(feature = "sqlite")]
pub(crate) fn uuid_blob(value: Uuid) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
pub(crate) fn uuid_from_blob(bytes: Vec<u8>) -> Result<Uuid, auction_application::RepositoryError> {
    Uuid::from_slice(&bytes).map_err(|_| repository_error_owned("Worker 调度 UUID 格式无效"))
}
