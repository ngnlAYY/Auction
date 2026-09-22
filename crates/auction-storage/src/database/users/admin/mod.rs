//! Users owner 的 Admin 用户管理入口与共享校验。
//!
//! 事务和 SQL 分别按 PostgreSQL/SQLite backend 拆到子文件；本模块只拥有应用端口分派、
//! 安全投影校验、命令 payload 解析和跨后端共享规则。

#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

use auction_application::{
    AdminRenameRevertWrite, AdminUser, AdminUserBlacklistHistoryEntry, AdminUserCommandKind,
    AdminUserCommandResult, AdminUserCreateWrite, AdminUserError, AdminUserKeyWrite,
    AdminUserListItem, AdminUserListRequest, AdminUserMutationAction, AdminUserMutationWrite,
    AdminUserPage, AdminUserStatus, AdminUserStore, ApplicationError, ApplicationFuture,
    MailLoginKeyEnvelope, RecoveryBuyerBlacklistCommand, RecoveryBuyerBlacklistResult,
    RecoveryBuyerBlacklister, RepositoryError, RequestHash,
};
#[cfg(feature = "postgres")]
use auction_application::{RECOVERY_BUYER_BLACKLIST_ACTOR, RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_domain::{CommandId, deterministic_uuid};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

#[cfg(feature = "sqlite")]
use sqlx::{SqliteConnection, sqlite::SqliteRow};
#[cfg(feature = "postgres")]
use sqlx::{postgres::PgRow, types::Json};

use super::DatabaseStore;

impl AdminUserStore for DatabaseStore {
    fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(AdminUserError::InvalidRequest));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => postgres::list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => sqlite::list_sqlite(store, request).await,
            }
        })
    }

    fn find_command(
        &self,
        command_id: Uuid,
        kind: AdminUserCommandKind,
        request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<AdminUserCommandResult>> {
        Box::pin(async move {
            if command_id.is_nil() || request_hash.as_bytes() == [0; 32] {
                return Err(admin_error(AdminUserError::InvalidRequest));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    postgres::find_command_postgres(store, command_id, kind, request_hash).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => {
                    sqlite::find_command_sqlite(store, command_id, kind, request_hash).await
                }
            }
        })
    }

    fn create(&self, write: AdminUserCreateWrite) -> ApplicationFuture<'_, AdminUserCommandResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => postgres::create_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => sqlite::create_sqlite(store, write).await,
            }
        })
    }

    fn mutate(
        &self,
        write: AdminUserMutationWrite,
    ) -> ApplicationFuture<'_, AdminUserCommandResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => postgres::mutate_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => sqlite::mutate_sqlite(store, write).await,
            }
        })
    }

    fn revert_rename(
        &self,
        write: AdminRenameRevertWrite,
    ) -> ApplicationFuture<'_, AdminUserCommandResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => postgres::revert_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => sqlite::revert_sqlite(store, write).await,
            }
        })
    }
}

impl RecoveryBuyerBlacklister for DatabaseStore {
    fn ensure_buyer_blacklisted(
        &self,
        command: RecoveryBuyerBlacklistCommand,
    ) -> ApplicationFuture<'_, RecoveryBuyerBlacklistResult> {
        Box::pin(async move {
            if !command.validate() {
                return Err(admin_error(AdminUserError::InvalidRequest));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    postgres::ensure_recovery_blacklist_postgres(store, command).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "recovery 买家黑名单仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredUserPayload {
    user: AdminUser,
}

fn admin_error(error: AdminUserError) -> ApplicationError {
    ApplicationError::AdminUser(error)
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

fn validate_create_write(write: &AdminUserCreateWrite) -> Result<(), ApplicationError> {
    if write.command_id.is_nil()
        || write.user_id.is_nil()
        || write.request_hash.as_bytes() == [0; 32]
        || !valid_text(&write.username, 256)
        || write.forum_account_id.as_deref().is_some_and(|value| !valid_forum_account_id(value))
        || !valid_text(&write.actor_id, 256)
        || write.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || !valid_time(write.now)
        || !valid_key_write(&write.key)
    {
        return Err(admin_error(AdminUserError::InvalidRequest));
    }
    Ok(())
}

fn validate_mutation_write(write: &AdminUserMutationWrite) -> Result<(), ApplicationError> {
    let fields_valid = match write.action {
        AdminUserMutationAction::Rename => {
            write.username.as_deref().is_some_and(|value| valid_text(value, 256))
                && write.forum_account_id.is_none()
                && write.public_reason.is_none()
                && write.key.is_none()
        }
        AdminUserMutationAction::SetForumAccountId => {
            write.forum_account_id.as_deref().is_some_and(valid_forum_account_id)
                && write.username.is_none()
                && write.public_reason.is_none()
                && write.key.is_none()
        }
        AdminUserMutationAction::Blacklist => {
            write.public_reason.as_deref().is_some_and(|value| valid_text(value, 1024))
                && write.username.is_none()
                && write.forum_account_id.is_none()
                && write.key.is_none()
        }
        AdminUserMutationAction::ReissueKey => {
            write.username.is_none()
                && write.forum_account_id.is_none()
                && write.public_reason.is_none()
                && write.key.as_ref().is_some_and(valid_key_write)
        }
        AdminUserMutationAction::RevokeKey | AdminUserMutationAction::Unblacklist => {
            write.username.is_none()
                && write.forum_account_id.is_none()
                && write.public_reason.is_none()
                && write.key.is_none()
        }
    };
    if write.command_id.is_nil()
        || write.user_id.is_nil()
        || write.expected_state_version <= 0
        || write.request_hash.as_bytes() == [0; 32]
        || !valid_text(&write.actor_id, 256)
        || write.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || !valid_time(write.now)
        || !fields_valid
    {
        return Err(admin_error(AdminUserError::InvalidRequest));
    }
    Ok(())
}

fn validate_revert_write(write: &AdminRenameRevertWrite) -> Result<(), ApplicationError> {
    if write.command_id.is_nil()
        || write.rename_id.is_nil()
        || write.expected_state_version <= 0
        || write.request_hash.as_bytes() == [0; 32]
        || !valid_text(&write.actor_id, 256)
        || write.request_id.as_deref().is_some_and(|value| !valid_text(value, 256))
        || !valid_time(write.now)
    {
        return Err(admin_error(AdminUserError::InvalidRequest));
    }
    Ok(())
}

fn valid_key_write(write: &AdminUserKeyWrite) -> bool {
    write.public_key_hash != [0; 32]
        && write.envelope.ciphertext().len() >= 16
        && write.envelope.nonce().len() == 12
        && valid_text(write.envelope.encryption_key_id(), 256)
        && valid_text(write.envelope.fingerprint(), 256)
        && write.response_envelope.as_ref().is_none_or(|envelope| {
            envelope.ciphertext().len() >= 16
                && envelope.nonce().len() == 12
                && valid_text(envelope.encryption_key_id(), 256)
                && valid_text(envelope.fingerprint(), 256)
        })
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_forum_account_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && bytes[0] != b'0'
        && bytes.iter().all(u8::is_ascii_digit)
}

fn valid_time(value: DateTime<Utc>) -> bool {
    value.timestamp_nanos_opt().is_some()
}

fn parse_payload(
    payload: Value,
    user_id: Uuid,
    response_envelope: Option<MailLoginKeyEnvelope>,
) -> Result<AdminUserCommandResult, ApplicationError> {
    let payload: StoredUserPayload = serde_json::from_value(payload)
        .map_err(|_| admin_error(AdminUserError::StoredResultCorrupt))?;
    if payload.user.id != user_id {
        return Err(admin_error(AdminUserError::StoredResultCorrupt));
    }
    let result = AdminUserCommandResult { user: payload.user, response_envelope };
    if result.validate() {
        Ok(result)
    } else {
        Err(admin_error(AdminUserError::StoredResultCorrupt))
    }
}

fn load_response_envelope(
    ciphertext: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
    key_id: Option<String>,
    fingerprint: Option<String>,
) -> Result<Option<MailLoginKeyEnvelope>, ApplicationError> {
    match (ciphertext, nonce, key_id, fingerprint) {
        (None, None, None, None) => Ok(None),
        (Some(ciphertext), Some(nonce), Some(key_id), Some(fingerprint)) => {
            MailLoginKeyEnvelope::new(ciphertext, nonce, key_id, fingerprint)
                .map(Some)
                .map_err(ApplicationError::from)
        }
        _ => Err(admin_error(AdminUserError::StoredResultCorrupt)),
    }
}
