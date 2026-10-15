//! Admin realtime 的统一存储错误映射。

use auction_application::{ApplicationError, RealtimeAdminError, RepositoryError};

pub(super) fn admin_error(error: RealtimeAdminError) -> ApplicationError {
    ApplicationError::RealtimeAdmin(error)
}

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
