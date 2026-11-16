//! 恢复事务的错误分类：保留 SQLSTATE 与资金冲突到重试边界，不匹配错误文本。

use crate::database::settlement::returns::ReturnTransactionError;
use crate::database::storage_error;
use crate::database::storage_error_from;

/// 邮箱维护恢复事务的错误。
pub(super) enum RecoveryError {
    Sqlx(sqlx::Error),
    Funding(auction_application::settlement::FundingError),
    Evidence(crate::MailEvidenceTransactionError),
    Rejected,
}

impl From<sqlx::Error> for RecoveryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

impl From<auction_application::settlement::FundingError> for RecoveryError {
    fn from(error: auction_application::settlement::FundingError) -> Self {
        Self::Funding(error)
    }
}

impl From<crate::MailEvidenceTransactionError> for RecoveryError {
    fn from(error: crate::MailEvidenceTransactionError) -> Self {
        Self::Evidence(error)
    }
}

/// 结算退件案件推进的错误：序列化冲突与资金 owner 分类原样保留到重试边界，业务拒绝与仓储错误
/// 一律按拒绝回滚，不暴露细节。
impl From<ReturnTransactionError> for RecoveryError {
    fn from(error: ReturnTransactionError) -> Self {
        match error {
            ReturnTransactionError::Sqlx(error) => Self::Sqlx(error),
            ReturnTransactionError::Funding(error) => Self::Funding(error),
            ReturnTransactionError::Business(_) | ReturnTransactionError::Repository(_) => {
                Self::Rejected
            }
        }
    }
}

impl RecoveryError {
    /// 仅在整事务重试结束后投影 SQLx 诊断，其他拒绝仍使用原固定原因。
    pub(super) fn into_application_error(
        self,
        context: &'static str,
    ) -> auction_application::core::ApplicationError {
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
            Self::Funding(auction_application::settlement::FundingError::SerializationConflict) => {
                true
            }
            Self::Evidence(error) => error.is_serialization_conflict(),
            _ => false,
        }
    }
}
