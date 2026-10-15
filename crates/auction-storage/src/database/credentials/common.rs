//! 凭据适配器共享错误、命令结果和安全文本校验。
//!
//! 该模块不执行数据库操作，只集中维护跨读取、写入和运行时端口共用的安全映射。

#[cfg(feature = "postgres")]
use auction_application::RepositoryError;
#[cfg(feature = "postgres")]
use auction_application::{AdminCredential, AdminCredentialDeleteResult};
use auction_application::{AdminCredentialError, ApplicationError};
#[cfg(feature = "postgres")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "postgres")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredCredentialPayload {
    pub(super) credential: Option<AdminCredential>,
    pub(super) deleted: Option<AdminCredentialDeleteResult>,
}

pub(super) fn admin_error(error: AdminCredentialError) -> ApplicationError {
    ApplicationError::AdminCredential(error)
}

#[cfg(feature = "postgres")]
pub(super) fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
