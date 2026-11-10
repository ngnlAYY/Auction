//! 外部凭据命令的 PostgreSQL 装配；凭据行、命令账本和追加审计原子提交。

use auction_application::admin::AdminCredentialEnvelope;
use auction_application::admin::{
    AdminCredentialCommandKind, AdminCredentialCommandResult, AdminCredentialDeleteWrite,
    AdminCredentialError, AdminCredentialHealthReader, AdminCredentialHealthSnapshot,
    AdminCredentialImportWrite, AdminCredentialKind, AdminCredentialListRequest,
    AdminCredentialMutationWrite, AdminCredentialPage, AdminCredentialRuntimeReader,
    AdminCredentialRuntimeRecord, AdminCredentialRuntimeWriter, AdminCredentialStore,
    AdminCredentialWorld,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::RequestHash;
use uuid::Uuid;

use super::DatabaseStore;

mod check;
mod common;
mod evidence;
pub use evidence::{
    MailEvidenceTransactionError, lock_mail_evidence_credentials, lock_mail_runtime_generation,
    validate_mail_evidence_time,
};
mod mapping;
mod ownership;
mod read;
mod runtime;
mod write;

use common::*;
use mapping::*;
pub use ownership::{CredentialRuntimeOwnership, CredentialRuntimeOwnershipError};
use read::*;
use runtime::*;
use write::*;

impl AdminCredentialStore for DatabaseStore {
    fn list(
        &self,
        request: AdminCredentialListRequest,
    ) -> ApplicationFuture<'_, AdminCredentialPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(AdminCredentialError::InvalidRequest));
            }
            list_postgres(self, request).await
        })
    }

    fn find_command(
        &self,
        command_id: Uuid,
        kind: AdminCredentialCommandKind,
        request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<AdminCredentialCommandResult>> {
        Box::pin(async move {
            if command_id.is_nil() || request_hash.as_bytes() == [0; 32] {
                return Err(admin_error(AdminCredentialError::InvalidRequest));
            }
            find_command_postgres(self, command_id, kind, request_hash).await
        })
    }

    fn import(
        &self,
        write: AdminCredentialImportWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move { import_postgres(self, write).await })
    }

    fn mutate(
        &self,
        write: AdminCredentialMutationWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move { mutate_postgres(self, write).await })
    }

    fn delete(
        &self,
        write: AdminCredentialDeleteWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move { delete_postgres(self, write).await })
    }
}

impl AdminCredentialRuntimeReader for DatabaseStore {
    fn list_active(
        &self,
        kind: AdminCredentialKind,
        world: AdminCredentialWorld,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialRuntimeRecord>> {
        Box::pin(async move { list_active_postgres(self, kind, world).await })
    }
}

impl AdminCredentialRuntimeWriter for DatabaseStore {
    fn mark_invalid(
        &self,
        credential_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if credential_id.is_nil() || now.timestamp_nanos_opt().is_none() {
                return Err(admin_error(AdminCredentialError::InvalidRequest));
            }
            mark_invalid_postgres(self, credential_id, now).await
        })
    }
}

impl AdminCredentialHealthReader for DatabaseStore {
    fn health(&self) -> ApplicationFuture<'_, AdminCredentialHealthSnapshot> {
        Box::pin(async move { health_postgres(self).await })
    }
}
