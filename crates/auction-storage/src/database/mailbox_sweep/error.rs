//! 恢复事务的错误分类：保留 SQLSTATE 与资金冲突到重试边界，不匹配错误文本。

use crate::database::storage_error;
use crate::database::storage_error_from;

/// 保留 PostgreSQL SQLSTATE 与资金冲突分类至重试边界，不匹配错误文本。
pub(super) enum RecoveryError {
    Sqlx(sqlx::Error),
    Funding(auction_application::FundingError),
    Evidence(crate::MailEvidenceTransactionError),
    Rejected,
}

impl From<sqlx::Error> for RecoveryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

impl From<auction_application::FundingError> for RecoveryError {
    fn from(error: auction_application::FundingError) -> Self {
        Self::Funding(error)
    }
}

impl From<crate::MailEvidenceTransactionError> for RecoveryError {
    fn from(error: crate::MailEvidenceTransactionError) -> Self {
        Self::Evidence(error)
    }
}

impl RecoveryError {
    /// 仅在整事务重试结束后投影 SQLx 诊断，其他拒绝仍使用原固定原因。
    pub(super) fn into_application_error(
        self,
        context: &'static str,
    ) -> auction_application::ApplicationError {
        match self {
            Self::Sqlx(error)
            | Self::Evidence(crate::MailEvidenceTransactionError::Sqlx(error)) => {
                storage_error_from(error, context)
            }
            Self::Funding(_)
            | Self::Evidence(crate::MailEvidenceTransactionError::Rejected(_))
            | Self::Rejected => storage_error(context),
        }
    }
    pub(super) fn retryable(&self) -> bool {
        match self {
            Self::Sqlx(error) => crate::database::observed_serialization_conflict(error),
            Self::Funding(auction_application::FundingError::SerializationConflict) => true,
            Self::Evidence(error) => error.is_serialization_conflict(),
            _ => false,
        }
    }
}
