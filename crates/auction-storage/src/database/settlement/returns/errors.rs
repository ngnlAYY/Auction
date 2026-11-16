//! 结算退件存储事务的错误分类：只有序列化冲突与死锁可在有界重试内重跑整次事务，业务结论与
//! 资金 owner 的类型化拒绝原样带出，SQL 原文不离开存储层。

use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::settlement::{FundingError, SettlementReturnError};

use crate::database::mail::delivery::MailDeliveryTransactionError;

/// 结算退件单次事务内的错误；邮箱巡检事务借用本模块推进案件时经 `From` 转换为自己的错误分类。
#[derive(Debug)]
pub(crate) enum ReturnTransactionError {
    Sqlx(sqlx::Error),
    Business(SettlementReturnError),
    Funding(FundingError),
    Repository(RepositoryError),
}

impl From<sqlx::Error> for ReturnTransactionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

impl From<SettlementReturnError> for ReturnTransactionError {
    fn from(error: SettlementReturnError) -> Self {
        Self::Business(error)
    }
}

impl From<FundingError> for ReturnTransactionError {
    fn from(error: FundingError) -> Self {
        Self::Funding(error)
    }
}

impl From<RepositoryError> for ReturnTransactionError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<MailDeliveryTransactionError> for ReturnTransactionError {
    fn from(error: MailDeliveryTransactionError) -> Self {
        match error {
            MailDeliveryTransactionError::Sqlx(error) => Self::Sqlx(error),
            MailDeliveryTransactionError::Rejected(error) => Self::Repository(error),
        }
    }
}

impl ReturnTransactionError {
    /// 有界重试闭集：SQL 层观察到的序列化冲突与死锁，或资金 owner 报告的序列化冲突。
    pub(super) fn retryable(&self) -> bool {
        match self {
            Self::Sqlx(error) => crate::database::observed_serialization_conflict(error),
            Self::Funding(FundingError::SerializationConflict) => true,
            _ => false,
        }
    }

    /// 离开存储层：业务错误保留类型化分类；资金与 SQL 失败只暴露安全分类。
    pub(super) fn into_application(self) -> ApplicationError {
        match self {
            Self::Sqlx(error) => crate::database::storage_failure(
                &error,
                "结算退件记录不存在",
                "结算退件数据库操作失败",
            )
            .into(),
            Self::Business(error) => error.into(),
            Self::Funding(_) => SettlementReturnError::Unavailable.into(),
            Self::Repository(error) => error.into(),
        }
    }
}
