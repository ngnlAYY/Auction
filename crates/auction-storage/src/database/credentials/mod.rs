//! 外部凭据命令的 PostgreSQL 装配；凭据行、命令账本和追加审计原子提交。

#[cfg(feature = "postgres")]
use auction_application::AdminCredentialEnvelope;
use auction_application::{
    AdminCredentialCommandKind, AdminCredentialCommandResult, AdminCredentialDeleteWrite,
    AdminCredentialError, AdminCredentialHealthReader, AdminCredentialHealthSnapshot,
    AdminCredentialImportWrite, AdminCredentialKind, AdminCredentialListRequest,
    AdminCredentialMutationWrite, AdminCredentialPage, AdminCredentialRuntimeReader,
    AdminCredentialRuntimeRecord, AdminCredentialRuntimeWriter, AdminCredentialStore,
    AdminCredentialWorld, ApplicationFuture, RequestHash,
};
use uuid::Uuid;

use super::DatabaseStore;

mod check;
mod common;
#[cfg(feature = "postgres")]
mod evidence;
#[cfg(feature = "postgres")]
pub use evidence::{
    MailEvidenceTransactionError, lock_mail_evidence_credentials, lock_mail_runtime_generation,
    validate_mail_evidence_time,
};
#[cfg(feature = "postgres")]
mod mapping;
#[cfg(feature = "postgres")]
mod ownership;
#[cfg(feature = "postgres")]
mod read;
#[cfg(feature = "postgres")]
mod runtime;
#[cfg(feature = "postgres")]
mod write;

use common::*;
#[cfg(feature = "postgres")]
use mapping::*;
#[cfg(feature = "postgres")]
pub use ownership::{CredentialRuntimeOwnership, CredentialRuntimeOwnershipError};
#[cfg(feature = "postgres")]
use read::*;
#[cfg(feature = "postgres")]
use runtime::*;
#[cfg(feature = "postgres")]
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_postgres(store, request).await,
            }
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    find_command_postgres(store, command_id, kind, request_hash).await
                }
            }
        })
    }

    fn import(
        &self,
        write: AdminCredentialImportWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => import_postgres(store, write).await,
            }
        })
    }

    fn mutate(
        &self,
        write: AdminCredentialMutationWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => mutate_postgres(store, write).await,
            }
        })
    }

    fn delete(
        &self,
        write: AdminCredentialDeleteWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => delete_postgres(store, write).await,
            }
        })
    }
}

impl AdminCredentialRuntimeReader for DatabaseStore {
    fn list_active(
        &self,
        kind: AdminCredentialKind,
        world: AdminCredentialWorld,
    ) -> ApplicationFuture<'_, Vec<AdminCredentialRuntimeRecord>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_active_postgres(store, kind, world).await,
            }
        })
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
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => mark_invalid_postgres(store, credential_id, now).await,
            }
        })
    }
}

impl AdminCredentialHealthReader for DatabaseStore {
    fn health(&self) -> ApplicationFuture<'_, AdminCredentialHealthSnapshot> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => health_postgres(store).await,
            }
        })
    }
}
