//! Admin 凭据 storage owner。
//!
//! 凭据只在 PostgreSQL 完整档落库。每个写命令使用 Serializable 事务和命令 advisory lock，
//! 将凭据行、命令账本、审计记录原子提交；SQLite Lite 显式报告能力不可用，不创建第二套
//! 不完整的凭据语义。

#[cfg(feature = "postgres")]
use auction_application::AdminCredentialEnvelope;
#[cfg(feature = "postgres")]
use auction_application::AdminCredentialHealthScope;
#[cfg(feature = "postgres")]
use auction_application::ApplicationError;
#[cfg(feature = "postgres")]
use auction_application::{AdminCredential, AdminCredentialDeleteResult, AdminCredentialStatus};
use auction_application::{
    AdminCredentialCommandKind, AdminCredentialCommandResult, AdminCredentialDeleteWrite,
    AdminCredentialError, AdminCredentialHealthReader, AdminCredentialHealthSnapshot,
    AdminCredentialImportWrite, AdminCredentialKind, AdminCredentialListRequest,
    AdminCredentialMutationWrite, AdminCredentialPage, AdminCredentialRuntimeReader,
    AdminCredentialRuntimeRecord, AdminCredentialRuntimeWriter, AdminCredentialStore,
    AdminCredentialWorld, ApplicationFuture, RequestHash,
};
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::{Value, json};
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

#[cfg(feature = "postgres")]
use sqlx::{Row, postgres::PgRow, types::Json};

use super::DatabaseStore;

mod common;
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
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
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
            #[cfg(feature = "sqlite")]
            let _ = &kind;
            if command_id.is_nil() || request_hash.as_bytes() == [0; 32] {
                return Err(admin_error(AdminCredentialError::InvalidRequest));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    find_command_postgres(store, command_id, kind, request_hash).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }

    fn import(
        &self,
        write: AdminCredentialImportWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = &write;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => import_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }

    fn mutate(
        &self,
        write: AdminCredentialMutationWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = &write;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => mutate_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }

    fn delete(
        &self,
        write: AdminCredentialDeleteWrite,
    ) -> ApplicationFuture<'_, AdminCredentialCommandResult> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = &write;
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => delete_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
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
            #[cfg(not(feature = "postgres"))]
            let _ = (kind, world);
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => list_active_postgres(store, kind, world).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
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
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
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
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_unavailable()),
            }
        })
    }
}
