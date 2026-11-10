//! MoogleMail delivery PostgreSQL 错误分类辅助。

use auction_application::core::RepositoryError;

/// 内部入队事务保留 SQLx 错误，交由调用方按自身事务策略识别 SQLSTATE；对外端口仍须经
/// `into_repository` 脱敏，不能输出底层数据库错误。
#[derive(Debug, thiserror::Error)]
pub(crate) enum MailDeliveryTransactionError {
    #[error("MoogleMail delivery 数据库操作失败")]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Rejected(#[from] RepositoryError),
}

impl MailDeliveryTransactionError {
    pub(super) fn into_repository(self) -> RepositoryError {
        match self {
            Self::Sqlx(error) => map_storage_error(error),
            Self::Rejected(error) => error,
        }
    }
}

pub(super) fn map_storage_error(error: sqlx::Error) -> RepositoryError {
    crate::database::serialization_failure(
        &error,
        "MoogleMail delivery 事务发生序列化冲突",
        "MoogleMail delivery outbox 操作失败",
    )
}

pub(super) use crate::database::repository_error;
