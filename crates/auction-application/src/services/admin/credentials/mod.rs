//! Admin 凭据管理用例服务的稳定出口。

use std::sync::Arc;

use crate::{
    AdminCredential, AdminCredentialDeleteCommand, AdminCredentialDeleteResult,
    AdminCredentialImportCommand, AdminCredentialListRequest, AdminCredentialPage,
    AdminCredentialPort, AdminCredentialRenameCommand, AdminCredentialSealer,
    AdminCredentialStatusCommand, AdminCredentialStore, ApplicationFuture,
};

use digest::{delete_hash, import_hash, rename_hash, status_hash};
use validation::{
    next_version, validate_delete, validate_import, validate_rename, validate_status,
};

mod check;
mod commands;

pub use check::AdminCredentialCheckService;
mod digest;
mod query;
mod validation;

/// Admin 凭据管理服务。
pub struct AdminCredentialService<S, E> {
    store: Arc<S>,
    sealer: Arc<E>,
}

impl<S, E> AdminCredentialService<S, E>
where
    S: AdminCredentialStore,
    E: AdminCredentialSealer,
{
    /// 绑定凭据 storage 与独立的凭据数据密钥适配器。
    pub fn new(store: Arc<S>, sealer: Arc<E>) -> Self {
        Self { store, sealer }
    }
}

impl<S, E> AdminCredentialPort for AdminCredentialService<S, E>
where
    S: AdminCredentialStore + 'static,
    E: AdminCredentialSealer + 'static,
{
    fn list(
        &self,
        request: AdminCredentialListRequest,
    ) -> ApplicationFuture<'_, AdminCredentialPage> {
        Box::pin(async move { AdminCredentialService::list(self, request).await })
    }

    fn import(
        &self,
        command: AdminCredentialImportCommand,
    ) -> ApplicationFuture<'_, AdminCredential> {
        Box::pin(async move { AdminCredentialService::import(self, command).await })
    }

    fn rename(
        &self,
        command: AdminCredentialRenameCommand,
    ) -> ApplicationFuture<'_, AdminCredential> {
        Box::pin(async move { AdminCredentialService::rename(self, command).await })
    }

    fn set_status(
        &self,
        command: AdminCredentialStatusCommand,
    ) -> ApplicationFuture<'_, AdminCredential> {
        Box::pin(async move { AdminCredentialService::set_status(self, command).await })
    }

    fn delete(
        &self,
        command: AdminCredentialDeleteCommand,
    ) -> ApplicationFuture<'_, AdminCredentialDeleteResult> {
        Box::pin(async move { AdminCredentialService::delete(self, command).await })
    }
}
