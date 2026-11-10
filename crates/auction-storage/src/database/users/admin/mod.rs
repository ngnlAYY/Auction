//! Admin 用户命令与查询的端口装配；事务和 SQL 位于 PostgreSQL 子模块。

mod postgres;

use auction_application::admin::{
    AdminRenameRevertWrite, AdminUser, AdminUserCommandKind, AdminUserCommandResult,
    AdminUserCreateWrite, AdminUserError, AdminUserKeyWrite, AdminUserListRequest,
    AdminUserMutationAction, AdminUserMutationWrite, AdminUserPage, AdminUserStore,
    RecoveryBuyerBlacklistCommand, RecoveryBuyerBlacklistResult, RecoveryBuyerBlacklister,
};
use auction_application::auction::ApplicationFuture;
use auction_application::core::{ApplicationError, RequestHash};
use auction_application::mail::MailLoginKeyEnvelope;
use auction_domain::valid_text;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::DatabaseStore;

impl AdminUserStore for DatabaseStore {
    fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(admin_error(AdminUserError::InvalidRequest));
            }
            postgres::list_postgres(self, request).await
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
            postgres::find_command_postgres(self, command_id, kind, request_hash).await
        })
    }

    fn create(&self, write: AdminUserCreateWrite) -> ApplicationFuture<'_, AdminUserCommandResult> {
        Box::pin(async move { postgres::create_postgres(self, write).await })
    }

    fn mutate(
        &self,
        write: AdminUserMutationWrite,
    ) -> ApplicationFuture<'_, AdminUserCommandResult> {
        Box::pin(async move { postgres::mutate_postgres(self, write).await })
    }

    fn revert_rename(
        &self,
        write: AdminRenameRevertWrite,
    ) -> ApplicationFuture<'_, AdminUserCommandResult> {
        Box::pin(async move { postgres::revert_postgres(self, write).await })
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
            postgres::ensure_recovery_blacklist_postgres(self, command).await
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

fn valid_forum_account_id(value: &str) -> bool {
    auction_domain::valid_positive_decimal(value, 32)
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
