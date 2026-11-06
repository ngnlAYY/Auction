//! Admin Mail owner 的应用和事务错误。

use thiserror::Error;

use crate::ports::core::TransactionPolicyError;

/// Admin 邮件应用错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminMailError {
    #[error("Admin 邮件查询参数无效")]
    InvalidRequest,
    #[error("Admin 邮件命令参数无效")]
    InvalidCommand,
    #[error("Admin 邮件命令与已有请求冲突")]
    CommandConflict,
    #[error("Admin 邮件安全投影无效")]
    InvalidProjection,
    #[error("Admin 邮件命令账本结果损坏")]
    StoredResultCorrupt,
}

/// Admin 邮件事务错误。
#[derive(Debug, Error)]
pub enum AdminMailTransactionError {
    #[error("邮件轮询状态不存在")]
    NotFound,
    #[error("邮件轮询状态版本已变化：当前版本 {current_state_version}")]
    StateConflict { current_state_version: i64 },
    #[error("邮件 Admin 命令账本发生唯一冲突")]
    Conflict,
    #[error("邮件 Admin 事务读取或写入失败: {0}")]
    Failed(String),
    #[error("邮件 Admin 事务发生可重试的序列化冲突")]
    SerializationConflict,
}

/// 邮件 Admin 事务 runner 的失败类型边界。
pub trait AdminMailTransactionFailure:
    From<AdminMailTransactionError> + From<TransactionPolicyError> + Send
{
    fn is_serialization_conflict(&self) -> bool;
}
