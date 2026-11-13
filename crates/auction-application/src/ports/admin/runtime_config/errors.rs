//! runtime 文档、文件与命令的错误分类。

use thiserror::Error;

/// runtime 文档和应用命令错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminRuntimeConfigError {
    #[error("runtime 配置文档无效：{0}")]
    InvalidDocument(String),
    #[error("runtime 配置命令无效：{0}")]
    InvalidCommand(String),
    #[error("runtime 配置 fingerprint 已过期")]
    StaleFingerprint,
    #[error("runtime 配置 command 冲突")]
    CommandConflict,
    #[error("runtime 配置更新结果未知")]
    OutcomeUnknown,
    #[error("runtime 配置需要人工核对")]
    ReconciliationRequired,
    #[error("runtime 配置持久化命令损坏")]
    StoredResultCorrupt,
}

/// runtime 文件适配器错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminRuntimeFileError {
    #[error("runtime 配置文件不可用：{0}")]
    Unavailable(String),
    #[error("runtime 配置文件冲突")]
    Conflict,
    #[error("runtime 配置文件原子替换结果未知")]
    OutcomeUnknown,
}

/// runtime 命令账本适配器错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminRuntimeCommandError {
    #[error("runtime 配置命令不存在")]
    NotFound,
    #[error("runtime 配置命令发生唯一冲突")]
    Conflict,
    #[error("runtime 配置命令存储失败：{0}")]
    Failed(String),
}
