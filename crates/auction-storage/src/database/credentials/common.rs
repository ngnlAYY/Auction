//! 凭据适配器共享错误、命令结果和安全文本校验；本模块不执行数据库操作，只维护共用安全映射。

pub(super) use crate::database::storage_error;
use auction_application::admin::AdminCredentialError;
#[cfg(feature = "postgres")]
use auction_application::admin::{AdminCredential, AdminCredentialDeleteResult};
use auction_application::core::ApplicationError;
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
