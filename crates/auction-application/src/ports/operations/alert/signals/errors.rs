//! operations alert 应用层的安全错误闭集。

use thiserror::Error;

/// operations alert 应用层错误；错误正文不能携带原始外部响应或收件人内容。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum OperationsAlertError {
    #[error("operations alert 参数无效")]
    Invalid,
    #[error("operations alert 来源不可用")]
    SourceUnavailable,
    #[error("operations alert 持久化结果无效")]
    StoredResultCorrupt,
    #[error("operations alert claim 已过期或 fencing 不匹配")]
    StaleClaim,
    #[error("operations alert 依赖身份不匹配")]
    IdentityConflict,
}
