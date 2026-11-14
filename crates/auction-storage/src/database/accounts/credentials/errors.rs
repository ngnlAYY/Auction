//! 凭据适配器共享的错误映射与命令结果载荷；不执行数据库操作。

pub(super) use crate::database::storage_error;
use auction_application::admin::AdminCredentialError;
use auction_application::admin::{AdminCredential, AdminCredentialDeleteResult};
use auction_application::core::ApplicationError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredCredentialPayload {
    pub(super) credential: Option<AdminCredential>,
    pub(super) deleted: Option<AdminCredentialDeleteResult>,
}

pub(super) fn admin_error(error: AdminCredentialError) -> ApplicationError {
    ApplicationError::AdminCredential(error)
}
