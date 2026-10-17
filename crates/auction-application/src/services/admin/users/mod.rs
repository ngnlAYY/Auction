//! Admin Users 用例服务的稳定出口。

use std::sync::Arc;

use crate::AdminUserStore;
use crate::{
    AdminLoginKeyIssuer, AdminLoginKeyOpener, AdminRenameRevertCommand, AdminUserCreateCommand,
    AdminUserListRequest, AdminUserMutationCommand, AdminUserMutationResult, AdminUserPage,
    AdminUserPort, ApplicationFuture,
};

mod commands;
mod digest;
mod query;
mod validation;

use digest::{create_hash, mutation_hash, revert_hash};
use validation::{validate_create, validate_mutation, validate_revert};

/// Admin Users owner 用例服务。
pub struct AdminUserService<S, I, O> {
    store: Arc<S>,
    issuer: Arc<I>,
    opener: Arc<O>,
}

impl<S, I, O> AdminUserService<S, I, O>
where
    S: AdminUserStore,
    I: AdminLoginKeyIssuer,
    O: AdminLoginKeyOpener,
{
    /// 建立绑定 Users storage、key issuer 和历史 envelope opener 的服务。
    pub fn new(store: Arc<S>, issuer: Arc<I>, opener: Arc<O>) -> Self {
        Self { store, issuer, opener }
    }
}

impl<S, I, O> AdminUserPort for AdminUserService<S, I, O>
where
    S: AdminUserStore + 'static,
    I: AdminLoginKeyIssuer + 'static,
    O: AdminLoginKeyOpener + 'static,
{
    fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage> {
        Box::pin(async move { AdminUserService::list(self, request).await })
    }

    fn create(
        &self,
        command: AdminUserCreateCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult> {
        Box::pin(async move { AdminUserService::create(self, command).await })
    }

    fn mutate(
        &self,
        command: AdminUserMutationCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult> {
        Box::pin(async move { AdminUserService::mutate(self, command).await })
    }

    fn revert_rename(
        &self,
        command: AdminRenameRevertCommand,
    ) -> ApplicationFuture<'_, AdminUserMutationResult> {
        Box::pin(async move { AdminUserService::revert_rename(self, command).await })
    }
}
