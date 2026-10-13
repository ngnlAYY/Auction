//! Admin 凭据值对象共享的业务错误。

use thiserror::Error;

/// Admin 凭据业务错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdminCredentialError {
    #[error("Admin 凭据命令参数无效")]
    InvalidRequest,
    #[error("Admin 凭据不存在")]
    NotFound,
    #[error("Admin 凭据唯一约束、标签或密文指纹冲突")]
    Conflict,
    #[error("凭据命令 ID 与已有请求负载冲突")]
    CommandConflict,
    #[error("凭据状态版本冲突，当前版本为 {current_version}")]
    VersionConflict { current_version: i64 },
    #[error("Admin 凭据 owner 返回结果损坏")]
    StoredResultCorrupt,
    #[error("凭据校验繁忙，请稍后重试")]
    CheckBusy,
    #[error("凭据校验能力暂不可用")]
    CheckUnavailable,
    #[error("Admin 凭据加密能力不可用")]
    CryptoUnavailable,
}
