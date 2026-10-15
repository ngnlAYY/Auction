//! 会话适配器共享存储错误映射。

use auction_application::{ApplicationError, RepositoryError};

pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
