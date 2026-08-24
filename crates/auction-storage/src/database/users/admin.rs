//! Users owner 的 Admin 用户管理写入适配器。
//!
//! Admin A28/A30/A31 的用户行、改名历史、黑名单、登录 key 信封、命令账本和审计记录由本模块
//! 共同拥有。PostgreSQL 使用 Serializable 事务和命令 advisory lock；SQLite Lite 使用
//! `BEGIN IMMEDIATE` 的单写者事务。两种后端都只把安全用户投影写入命令账本，绝不持久化 key
//! 明文。

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
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

#[cfg(feature = "sqlite")]
use sqlx::{SqliteConnection, pool::PoolConnection, sqlite::SqliteRow};
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
                Self::Postgres(store) => list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => list_sqlite(store, request).await,
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
                    find_command_postgres(store, command_id, kind, request_hash).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => {
                    find_command_sqlite(store, command_id, kind, request_hash).await
                }
            }
        })
    }

    fn create(&self, write: AdminUserCreateWrite) -> ApplicationFuture<'_, AdminUserCommandResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => create_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => create_sqlite(store, write).await,
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
                Self::Postgres(store) => mutate_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => mutate_sqlite(store, write).await,
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
                Self::Postgres(store) => revert_postgres(store, write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => revert_sqlite(store, write).await,
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
                Self::Postgres(store) => ensure_recovery_blacklist_postgres(store, command).await,
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

#[cfg(feature = "postgres")]
async fn ensure_recovery_blacklist_postgres(
    store: &super::super::PostgresStore,
    command: RecoveryBuyerBlacklistCommand,
) -> Result<RecoveryBuyerBlacklistResult, ApplicationError> {
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, command.command_id).await?;

    if let Some(result) = load_command_postgres_tx(
        &mut transaction,
        command.command_id,
        AdminUserCommandKind::Blacklist,
        command.request_hash,
    )
    .await?
    {
        if result.user.id != command.buyer_user_id
            || result.user.status != AdminUserStatus::Blacklisted
        {
            return Err(admin_error(AdminUserError::StoredResultCorrupt));
        }
        let result = recovery_blacklist_result(&command, result.user)?;
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    let before = load_user_postgres_tx(&mut transaction, command.buyer_user_id).await?;
    if command.occurred_at < before.created_at {
        return Err(admin_error(AdminUserError::InvalidRequest));
    }
    if before.status == AdminUserStatus::Active {
        sqlx::query(
            "UPDATE public.users
             SET status = 'blacklisted', state_version = state_version + 1, updated_at = $1
             WHERE id = $2",
        )
        .bind(command.occurred_at)
        .bind(command.buyer_user_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_admin_write_error)?;
        sqlx::query(
            "INSERT INTO public.user_blacklist
                (user_id, public_reason, actor, created_at, revoked_at)
             VALUES ($1, $2, $3, $4, NULL)
             ON CONFLICT (user_id) DO UPDATE
             SET public_reason = EXCLUDED.public_reason,
                 actor = EXCLUDED.actor,
                 created_at = EXCLUDED.created_at,
                 revoked_at = NULL",
        )
        .bind(command.buyer_user_id)
        .bind(RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON)
        .bind(RECOVERY_BUYER_BLACKLIST_ACTOR)
        .bind(command.occurred_at)
        .execute(&mut *transaction)
        .await
        .map_err(map_admin_write_error)?;
        sqlx::query(
            "INSERT INTO public.user_blacklist_history
                (id, user_id, public_reason, actor, blacklisted_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(command.command_id)
        .bind(command.buyer_user_id)
        .bind(RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON)
        .bind(RECOVERY_BUYER_BLACKLIST_ACTOR)
        .bind(command.occurred_at)
        .execute(&mut *transaction)
        .await
        .map_err(map_admin_write_error)?;
    }

    let user = load_user_postgres_tx(&mut transaction, command.buyer_user_id).await?;
    if user.status != AdminUserStatus::Blacklisted {
        return Err(admin_error(AdminUserError::StoredResultCorrupt));
    }
    let command_result = AdminUserCommandResult { user: user.clone(), response_envelope: None };
    insert_command_postgres(
        &mut transaction,
        command.command_id,
        AdminUserCommandKind::Blacklist,
        command.request_hash,
        &command_result,
        command.buyer_user_id,
        command.occurred_at,
    )
    .await?;
    insert_audit_postgres(
        &mut transaction,
        command.command_id,
        AdminUserCommandKind::Blacklist,
        command.buyer_user_id,
        RECOVERY_BUYER_BLACKLIST_ACTOR,
        None,
        command.occurred_at,
    )
    .await?;
    let result = recovery_blacklist_result(&command, user)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

#[cfg(feature = "postgres")]
fn recovery_blacklist_result(
    command: &RecoveryBuyerBlacklistCommand,
    user: AdminUser,
) -> Result<RecoveryBuyerBlacklistResult, ApplicationError> {
    let result = RecoveryBuyerBlacklistResult {
        command_id: command.command_id,
        recovery_id: command.recovery_id,
        action_id: command.action_id,
        buyer_user_id: command.buyer_user_id,
        user,
        public_reason: command.public_reason.clone(),
        actor_id: command.actor_id.clone(),
        occurred_at: command.occurred_at,
    };
    result
        .validate()
        .then_some(result)
        .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))
}

fn validate_create_write(write: &AdminUserCreateWrite) -> Result<(), ApplicationError> {
    if write.command_id.is_nil()
        || write.user_id.is_nil()
        || write.request_hash.as_bytes() == [0; 32]
        || !valid_text(&write.username, 256)
        || !valid_text(&write.game_player_id, 256)
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
                && write.game_player_id.is_none()
                && write.public_reason.is_none()
                && write.key.is_none()
        }
        AdminUserMutationAction::SetGamePlayerId => {
            write.game_player_id.as_deref().is_some_and(|value| valid_text(value, 256))
                && write.username.is_none()
                && write.public_reason.is_none()
                && write.key.is_none()
        }
        AdminUserMutationAction::Blacklist => {
            write.public_reason.as_deref().is_some_and(|value| valid_text(value, 1024))
                && write.username.is_none()
                && write.game_player_id.is_none()
                && write.key.is_none()
        }
        AdminUserMutationAction::ReissueKey => {
            write.username.is_none()
                && write.game_player_id.is_none()
                && write.public_reason.is_none()
                && write.key.as_ref().is_some_and(valid_key_write)
        }
        AdminUserMutationAction::RevokeKey | AdminUserMutationAction::Unblacklist => {
            write.username.is_none()
                && write.game_player_id.is_none()
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

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::super::PostgresStore,
    request: AdminUserListRequest,
) -> Result<AdminUserPage, ApplicationError> {
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
    let name = request.name.as_deref().filter(|value| !value.is_empty());
    let status = request.status.map(AdminUserStatus::as_str);
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM public.users
         WHERE ($1::text IS NULL OR strpos(lower(username), lower($1)) > 0
                OR strpos(lower(game_player_id), lower($1)) > 0)
           AND ($2::text IS NULL OR status = $2)",
    )
    .bind(name)
    .bind(status)
    .fetch_one(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    let rows = sqlx::query(
        "SELECT id, username, game_player_id, status, key_status, state_version,
                created_at, updated_at
         FROM public.users
         WHERE ($1::text IS NULL OR strpos(lower(username), lower($1)) > 0
                OR strpos(lower(game_player_id), lower($1)) > 0)
           AND ($2::text IS NULL OR status = $2)
         ORDER BY lower(username), id
         LIMIT $3 OFFSET $4",
    )
    .bind(name)
    .bind(status)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    let mut users = Vec::with_capacity(rows.len());
    for row in rows {
        let user = map_postgres_user(&row)?;
        let (blacklist_count, blacklist_history) =
            load_blacklist_history_postgres(&mut transaction, user.id).await?;
        users.push(AdminUserListItem { user, blacklist_count, blacklist_history });
    }
    transaction.commit().await.map_err(map_storage_error)?;
    let page = AdminUserPage { users, total, limit: request.limit, offset: request.offset };
    if page.validate(&request) {
        Ok(page)
    } else {
        Err(admin_error(AdminUserError::StoredResultCorrupt))
    }
}

#[cfg(feature = "postgres")]
async fn load_blacklist_history_postgres(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<(i64, Vec<AdminUserBlacklistHistoryEntry>), ApplicationError> {
    let rows = sqlx::query(
        "SELECT public_reason, blacklisted_at, count(*) OVER () AS total_count
         FROM public.user_blacklist_history
         WHERE user_id = $1
         ORDER BY blacklisted_at DESC, id DESC
         LIMIT 10",
    )
    .bind(user_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    let blacklist_count = rows
        .first()
        .map(|row| row.try_get("total_count"))
        .transpose()
        .map_err(map_storage_error)?
        .unwrap_or(0);
    let mut blacklist_history = Vec::with_capacity(rows.len());
    for row in rows {
        blacklist_history.push(AdminUserBlacklistHistoryEntry {
            public_reason: row.try_get("public_reason").map_err(map_storage_error)?,
            blacklisted_at: row.try_get("blacklisted_at").map_err(map_storage_error)?,
        });
    }
    Ok((blacklist_count, blacklist_history))
}

#[cfg(feature = "postgres")]
async fn find_command_postgres(
    store: &super::super::PostgresStore,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
) -> Result<Option<AdminUserCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT command_kind, request_digest, user_id, result_payload,
                response_ciphertext, response_nonce, response_encryption_key_id,
                response_fingerprint
         FROM public.user_admin_commands WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_optional(store.pool())
    .await
    .map_err(map_storage_error)?;
    row.map(|row| parse_postgres_command(&row, command_id, kind, request_hash)).transpose()
}

#[cfg(feature = "postgres")]
async fn create_postgres(
    store: &super::super::PostgresStore,
    write: AdminUserCreateWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_create_write(&write)?;
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = load_command_postgres_tx(
        &mut transaction,
        write.command_id,
        AdminUserCommandKind::Create,
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    sqlx::query(
        "INSERT INTO public.users
            (id, username, status, public_key_hash, game_player_id,
             key_status, state_version, created_at, updated_at)
         VALUES ($1, $2, 'active', $3, $4, 'active', 1, $5, $5)",
    )
    .bind(write.user_id)
    .bind(&write.username)
    .bind(write.key.public_key_hash.to_vec())
    .bind(&write.game_player_id)
    .bind(write.now)
    .execute(&mut *transaction)
    .await
    .map_err(map_admin_write_error)?;
    insert_envelope_postgres(&mut transaction, write.user_id, &write.key.envelope, write.now)
        .await?;
    let user = load_user_postgres_tx(&mut transaction, write.user_id).await?;
    let result = AdminUserCommandResult { user, response_envelope: write.key.response_envelope };
    insert_command_postgres(
        &mut transaction,
        write.command_id,
        AdminUserCommandKind::Create,
        write.request_hash,
        &result,
        write.user_id,
        write.now,
    )
    .await?;
    insert_audit_postgres(
        &mut transaction,
        write.command_id,
        AdminUserCommandKind::Create,
        write.user_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn mutate_postgres(
    store: &super::super::PostgresStore,
    write: AdminUserMutationWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_mutation_write(&write)?;
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = load_command_postgres_tx(
        &mut transaction,
        write.command_id,
        write.action.kind(),
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    let before = load_user_postgres_tx(&mut transaction, write.user_id).await?;
    if before.state_version != write.expected_state_version {
        return Err(admin_error(AdminUserError::VersionConflict {
            current_version: before.state_version,
        }));
    }
    apply_mutation_postgres(&mut transaction, &write, &before).await?;
    let user = load_user_postgres_tx(&mut transaction, write.user_id).await?;
    let result = AdminUserCommandResult {
        user,
        response_envelope: write.key.as_ref().and_then(|key| key.response_envelope.clone()),
    };
    insert_command_postgres(
        &mut transaction,
        write.command_id,
        write.action.kind(),
        write.request_hash,
        &result,
        write.user_id,
        write.now,
    )
    .await?;
    insert_audit_postgres(
        &mut transaction,
        write.command_id,
        write.action.kind(),
        write.user_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn apply_mutation_postgres(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    write: &AdminUserMutationWrite,
    before: &AdminUser,
) -> Result<(), ApplicationError> {
    let id = write.user_id;
    let now = write.now;
    match write.action {
        AdminUserMutationAction::Rename => {
            let username = write.username.as_deref().ok_or(AdminUserError::InvalidRequest)?;
            if before.username == username {
                return Err(admin_error(AdminUserError::Conflict));
            }
            sqlx::query(
                "INSERT INTO public.user_renames
                    (id, user_id, previous_username, next_username, actor, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(write.command_id)
            .bind(id)
            .bind(&before.username)
            .bind(username)
            .bind(&write.actor_id)
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(map_admin_write_error)?;
            sqlx::query(
                "UPDATE public.users
                 SET username = $1, state_version = state_version + 1, updated_at = $2
                 WHERE id = $3",
            )
            .bind(username)
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_admin_write_error)?;
        }
        AdminUserMutationAction::SetGamePlayerId => {
            let game_player_id =
                write.game_player_id.as_deref().ok_or(AdminUserError::InvalidRequest)?;
            if before.game_player_id.as_deref() == Some(game_player_id) {
                return Err(admin_error(AdminUserError::Conflict));
            }
            sqlx::query(
                "UPDATE public.users
                 SET game_player_id = $1, state_version = state_version + 1, updated_at = $2
                 WHERE id = $3",
            )
            .bind(game_player_id)
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_admin_write_error)?;
        }
        AdminUserMutationAction::RevokeKey => {
            sqlx::query(
                "UPDATE public.users
                 SET key_status = 'revoked', state_version = state_version + 1, updated_at = $1
                 WHERE id = $2",
            )
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
        }
        AdminUserMutationAction::ReissueKey => {
            let key = write.key.as_ref().ok_or(AdminUserError::InvalidRequest)?;
            sqlx::query(
                "UPDATE public.users
                 SET public_key_hash = $1, key_status = 'active',
                     state_version = state_version + 1, updated_at = $2
                 WHERE id = $3",
            )
            .bind(key.public_key_hash.to_vec())
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_admin_write_error)?;
            insert_envelope_postgres(transaction, id, &key.envelope, now).await?;
        }
        AdminUserMutationAction::Blacklist => {
            if before.status == AdminUserStatus::Blacklisted {
                return Err(admin_error(AdminUserError::Conflict));
            }
            let reason = write.public_reason.as_deref().ok_or(AdminUserError::InvalidRequest)?;
            sqlx::query(
                "UPDATE public.users
                 SET status = 'blacklisted', state_version = state_version + 1, updated_at = $1
                 WHERE id = $2",
            )
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
            sqlx::query(
                "INSERT INTO public.user_blacklist
                    (user_id, public_reason, actor, created_at, revoked_at)
                 VALUES ($1, $2, $3, $4, NULL)
                 ON CONFLICT (user_id) DO UPDATE
                 SET public_reason = EXCLUDED.public_reason,
                     actor = EXCLUDED.actor, created_at = EXCLUDED.created_at,
                     revoked_at = NULL",
            )
            .bind(id)
            .bind(reason)
            .bind(&write.actor_id)
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(map_admin_write_error)?;
            sqlx::query(
                "INSERT INTO public.user_blacklist_history
                    (id, user_id, public_reason, actor, blacklisted_at)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(write.command_id)
            .bind(id)
            .bind(reason)
            .bind(&write.actor_id)
            .bind(now)
            .execute(&mut **transaction)
            .await
            .map_err(map_admin_write_error)?;
        }
        AdminUserMutationAction::Unblacklist => {
            if before.status != AdminUserStatus::Blacklisted {
                return Err(admin_error(AdminUserError::Conflict));
            }
            sqlx::query(
                "UPDATE public.users
                 SET status = 'active', state_version = state_version + 1, updated_at = $1
                 WHERE id = $2",
            )
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
            sqlx::query(
                "UPDATE public.user_blacklist
                 SET revoked_at = $1 WHERE user_id = $2 AND revoked_at IS NULL",
            )
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
        }
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn revert_postgres(
    store: &super::super::PostgresStore,
    write: AdminRenameRevertWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_revert_write(&write)?;
    let mut transaction = store.pool().begin().await.map_err(map_storage_error)?;
    set_serializable(&mut transaction).await?;
    lock_command(&mut transaction, write.command_id).await?;
    if let Some(result) = load_command_postgres_tx(
        &mut transaction,
        write.command_id,
        AdminUserCommandKind::RenameRevert,
        write.request_hash,
    )
    .await?
    {
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }
    let rename = sqlx::query(
        "SELECT user_id, previous_username, next_username, reverted_at
         FROM public.user_renames WHERE id = $1 FOR UPDATE",
    )
    .bind(write.rename_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or(AdminUserError::NotFound)?;
    let user_id: Uuid = rename.try_get("user_id").map_err(map_storage_error)?;
    let previous_username: String =
        rename.try_get("previous_username").map_err(map_storage_error)?;
    let next_username: String = rename.try_get("next_username").map_err(map_storage_error)?;
    let reverted_at: Option<DateTime<Utc>> =
        rename.try_get("reverted_at").map_err(map_storage_error)?;
    if reverted_at.is_some() {
        return Err(admin_error(AdminUserError::RenameNotRevertable));
    }
    let before = load_user_postgres_tx(&mut transaction, user_id).await?;
    if before.state_version != write.expected_state_version {
        return Err(admin_error(AdminUserError::VersionConflict {
            current_version: before.state_version,
        }));
    }
    if before.username != next_username {
        return Err(admin_error(AdminUserError::RenameNotRevertable));
    }
    sqlx::query(
        "UPDATE public.users
         SET username = $1, state_version = state_version + 1, updated_at = $2
         WHERE id = $3",
    )
    .bind(&previous_username)
    .bind(write.now)
    .bind(user_id)
    .execute(&mut *transaction)
    .await
    .map_err(map_admin_write_error)?;
    sqlx::query("UPDATE public.user_renames SET reverted_at = $1 WHERE id = $2")
        .bind(write.now)
        .bind(write.rename_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
    let result = AdminUserCommandResult {
        user: load_user_postgres_tx(&mut transaction, user_id).await?,
        response_envelope: None,
    };
    insert_command_postgres(
        &mut transaction,
        write.command_id,
        AdminUserCommandKind::RenameRevert,
        write.request_hash,
        &result,
        user_id,
        write.now,
    )
    .await?;
    insert_audit_postgres(
        &mut transaction,
        write.command_id,
        AdminUserCommandKind::RenameRevert,
        user_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn load_user_postgres_tx(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<AdminUser, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, username, game_player_id, status, key_status, state_version,
                created_at, updated_at
         FROM public.users WHERE id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or(AdminUserError::NotFound)?;
    map_postgres_user(&row)
}

#[cfg(feature = "postgres")]
fn map_postgres_user(row: &PgRow) -> Result<AdminUser, ApplicationError> {
    let status: String = row.try_get("status").map_err(map_storage_error)?;
    let key_status: String = row.try_get("key_status").map_err(map_storage_error)?;
    let user = AdminUser {
        id: row.try_get("id").map_err(map_storage_error)?,
        username: row.try_get("username").map_err(map_storage_error)?,
        game_player_id: row.try_get("game_player_id").map_err(map_storage_error)?,
        status: AdminUserStatus::parse(&status)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?,
        key_status: auction_application::AdminUserKeyStatus::parse(&key_status)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?,
        state_version: row.try_get("state_version").map_err(map_storage_error)?,
        created_at: row.try_get("created_at").map_err(map_storage_error)?,
        updated_at: row.try_get("updated_at").map_err(map_storage_error)?,
    };
    user.validate().then_some(user).ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))
}

#[cfg(feature = "postgres")]
async fn load_command_postgres_tx(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
) -> Result<Option<AdminUserCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT command_kind, request_digest, user_id, result_payload,
                response_ciphertext, response_nonce, response_encryption_key_id,
                response_fingerprint
         FROM public.user_admin_commands WHERE command_id = $1 FOR UPDATE",
    )
    .bind(command_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    row.map(|row| parse_postgres_command(&row, command_id, kind, request_hash)).transpose()
}

#[cfg(feature = "postgres")]
fn parse_postgres_command(
    row: &PgRow,
    _command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
) -> Result<AdminUserCommandResult, ApplicationError> {
    let stored_kind: String = row.try_get("command_kind").map_err(map_storage_error)?;
    let stored_hash: Vec<u8> = row.try_get("request_digest").map_err(map_storage_error)?;
    if stored_kind != kind.as_str() || stored_hash.as_slice() != request_hash.as_bytes() {
        return Err(admin_error(AdminUserError::CommandConflict));
    }
    let user_id: Uuid = row.try_get("user_id").map_err(map_storage_error)?;
    let payload: Json<Value> = row.try_get("result_payload").map_err(map_storage_error)?;
    let response = load_response_envelope(
        row.try_get("response_ciphertext").map_err(map_storage_error)?,
        row.try_get("response_nonce").map_err(map_storage_error)?,
        row.try_get("response_encryption_key_id").map_err(map_storage_error)?,
        row.try_get("response_fingerprint").map_err(map_storage_error)?,
    )?;
    parse_payload(payload.0, user_id, response)
}

#[cfg(feature = "postgres")]
async fn insert_envelope_postgres(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    envelope: &MailLoginKeyEnvelope,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.user_login_key_envelopes
            (user_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $6)
         ON CONFLICT (user_id) DO UPDATE
         SET ciphertext = EXCLUDED.ciphertext, nonce = EXCLUDED.nonce,
             encryption_key_id = EXCLUDED.encryption_key_id,
             fingerprint = EXCLUDED.fingerprint, updated_at = EXCLUDED.updated_at",
    )
    .bind(user_id)
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(envelope.fingerprint())
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_command_postgres(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
    result: &AdminUserCommandResult,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let response = result.response_envelope.as_ref();
    sqlx::query(
        "INSERT INTO public.user_admin_commands
            (command_id, command_kind, request_digest, user_id, result_payload,
             response_ciphertext, response_nonce, response_encryption_key_id,
             response_fingerprint, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(command_id)
    .bind(kind.as_str())
    .bind(request_hash.as_bytes().to_vec())
    .bind(user_id)
    .bind(Json(json!({ "user": &result.user })))
    .bind(response.map(|value| value.ciphertext().to_vec()))
    .bind(response.map(|value| value.nonce().to_vec()))
    .bind(response.map(|value| value.encryption_key_id().to_owned()))
    .bind(response.map(|value| value.fingerprint().to_owned()))
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_audit_postgres(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    user_id: Uuid,
    actor_id: &str,
    request_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES ($1, $2, 'admin', $3, $4, 'user', $5, 'success', $6, 'admin', $7)",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(actor_id)
    .bind(format!("user.{}", kind.as_str()))
    .bind(user_id.to_string())
    .bind(request_id)
    .bind(Json(json!({ "command_id": command_id, "command_kind": kind.as_str() })))
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn set_serializable(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), ApplicationError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn lock_command(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command_id: Uuid,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(command_id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    Ok(())
}

#[cfg(feature = "postgres")]
fn map_postgres_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("23505") =>
        {
            admin_error(AdminUserError::Conflict)
        }
        _ => map_storage_error(error),
    }
}

#[cfg(feature = "postgres")]
fn map_admin_write_error(error: sqlx::Error) -> ApplicationError {
    map_postgres_error(error)
}

#[cfg(feature = "postgres")]
fn map_storage_error(error: sqlx::Error) -> ApplicationError {
    let message = match &error {
        sqlx::Error::Database(database_error)
            if database_error.code().as_deref() == Some("40001") =>
        {
            "Admin Users 事务发生序列化冲突"
        }
        _ => "Admin Users 存储操作失败",
    };
    storage_error(message)
}

// SQLite 实现位于下一段；先保留统一 trait owner，避免 HTTP/应用层感知具体数据库类型。

#[cfg(feature = "sqlite")]
async fn list_sqlite(
    store: &super::super::SqliteStore,
    request: AdminUserListRequest,
) -> Result<AdminUserPage, ApplicationError> {
    let mut transaction = store.pool().begin().await.map_err(map_sqlite_error)?;
    let name = request.name.as_deref().filter(|value| !value.is_empty());
    let status = request.status.map(AdminUserStatus::as_str);
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users
         WHERE (? IS NULL OR instr(lower(username), lower(?)) > 0
                OR instr(lower(game_player_id), lower(?)) > 0)
           AND (? IS NULL OR status = ?)",
    )
    .bind(name)
    .bind(name)
    .bind(name)
    .bind(status)
    .bind(status)
    .fetch_one(&mut *transaction)
    .await
    .map_err(map_sqlite_error)?;
    let rows = sqlx::query(
        "SELECT id, username, game_player_id, status, key_status, state_version,
                created_at, updated_at
         FROM users
         WHERE (? IS NULL OR instr(lower(username), lower(?)) > 0
                OR instr(lower(game_player_id), lower(?)) > 0)
           AND (? IS NULL OR status = ?)
         ORDER BY lower(username), id
         LIMIT ? OFFSET ?",
    )
    .bind(name)
    .bind(name)
    .bind(name)
    .bind(status)
    .bind(status)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_sqlite_error)?;
    let mut users = Vec::with_capacity(rows.len());
    for row in rows {
        let user = map_sqlite_user(&row)?;
        let (blacklist_count, blacklist_history) =
            load_blacklist_history_sqlite(&mut transaction, user.id).await?;
        users.push(AdminUserListItem { user, blacklist_count, blacklist_history });
    }
    transaction.commit().await.map_err(map_sqlite_error)?;
    let page = AdminUserPage { users, total, limit: request.limit, offset: request.offset };
    page.validate(&request)
        .then_some(page)
        .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))
}

#[cfg(feature = "sqlite")]
async fn load_blacklist_history_sqlite(
    connection: &mut SqliteConnection,
    user_id: Uuid,
) -> Result<(i64, Vec<AdminUserBlacklistHistoryEntry>), ApplicationError> {
    let rows = sqlx::query(
        "SELECT public_reason, blacklisted_at, count(*) OVER () AS total_count
         FROM user_blacklist_history
         WHERE user_id = ?
         ORDER BY blacklisted_at DESC, id DESC
         LIMIT 10",
    )
    .bind(uuid_blob(user_id))
    .fetch_all(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    let blacklist_count = rows
        .first()
        .map(|row| row.try_get("total_count"))
        .transpose()
        .map_err(map_sqlite_error)?
        .unwrap_or(0);
    let mut blacklist_history = Vec::with_capacity(rows.len());
    for row in rows {
        blacklist_history.push(AdminUserBlacklistHistoryEntry {
            public_reason: row.try_get("public_reason").map_err(map_sqlite_error)?,
            blacklisted_at: parse_sqlite_timestamp(
                row.try_get::<String, _>("blacklisted_at").map_err(map_sqlite_error)?,
            )?,
        });
    }
    Ok((blacklist_count, blacklist_history))
}

#[cfg(feature = "sqlite")]
async fn find_command_sqlite(
    store: &super::super::SqliteStore,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
) -> Result<Option<AdminUserCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT command_kind, request_digest, user_id, result_payload,
                response_ciphertext, response_nonce, response_encryption_key_id,
                response_fingerprint
         FROM user_admin_commands WHERE command_id = ?",
    )
    .bind(uuid_blob(command_id))
    .fetch_optional(store.pool())
    .await
    .map_err(map_sqlite_error)?;
    row.map(|row| parse_sqlite_command(&row, kind, request_hash)).transpose()
}

#[cfg(feature = "sqlite")]
async fn create_sqlite(
    store: &super::super::SqliteStore,
    write: AdminUserCreateWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_create_write(&write)?;
    let mut connection = begin_sqlite_write(store).await?;
    let result = create_sqlite_inner(&mut connection, write).await;
    finish_sqlite_write(connection, result).await
}

#[cfg(feature = "sqlite")]
async fn create_sqlite_inner(
    connection: &mut SqliteConnection,
    write: AdminUserCreateWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    if let Some(result) = load_command_sqlite(
        connection,
        write.command_id,
        AdminUserCommandKind::Create,
        write.request_hash,
    )
    .await?
    {
        return Ok(result);
    }
    sqlx::query(
        "INSERT INTO users
            (id, username, status, public_key_hash, game_player_id,
             key_status, state_version, created_at, updated_at)
         VALUES (?, ?, 'active', ?, ?, 'active', 1, ?, ?)",
    )
    .bind(uuid_blob(write.user_id))
    .bind(&write.username)
    .bind(write.key.public_key_hash.to_vec())
    .bind(&write.game_player_id)
    .bind(sqlite_timestamp(write.now))
    .bind(sqlite_timestamp(write.now))
    .execute(&mut *connection)
    .await
    .map_err(map_sqlite_write_error)?;
    insert_envelope_sqlite(connection, write.user_id, &write.key.envelope, write.now).await?;
    let user = load_user_sqlite(connection, write.user_id).await?;
    let result = AdminUserCommandResult { user, response_envelope: write.key.response_envelope };
    insert_command_sqlite(
        connection,
        write.command_id,
        AdminUserCommandKind::Create,
        write.request_hash,
        &result,
        write.user_id,
        write.now,
    )
    .await?;
    insert_audit_sqlite(
        connection,
        write.command_id,
        AdminUserCommandKind::Create,
        write.user_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    Ok(result)
}

#[cfg(feature = "sqlite")]
async fn mutate_sqlite(
    store: &super::super::SqliteStore,
    write: AdminUserMutationWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_mutation_write(&write)?;
    let mut connection = begin_sqlite_write(store).await?;
    let result = mutate_sqlite_inner(&mut connection, write).await;
    finish_sqlite_write(connection, result).await
}

#[cfg(feature = "sqlite")]
async fn mutate_sqlite_inner(
    connection: &mut SqliteConnection,
    write: AdminUserMutationWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    if let Some(result) =
        load_command_sqlite(connection, write.command_id, write.action.kind(), write.request_hash)
            .await?
    {
        return Ok(result);
    }
    let before = load_user_sqlite(connection, write.user_id).await?;
    if before.state_version != write.expected_state_version {
        return Err(admin_error(AdminUserError::VersionConflict {
            current_version: before.state_version,
        }));
    }
    apply_mutation_sqlite(connection, &write, &before).await?;
    let result = AdminUserCommandResult {
        user: load_user_sqlite(connection, write.user_id).await?,
        response_envelope: write.key.as_ref().and_then(|key| key.response_envelope.clone()),
    };
    insert_command_sqlite(
        connection,
        write.command_id,
        write.action.kind(),
        write.request_hash,
        &result,
        write.user_id,
        write.now,
    )
    .await?;
    insert_audit_sqlite(
        connection,
        write.command_id,
        write.action.kind(),
        write.user_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    Ok(result)
}

#[cfg(feature = "sqlite")]
async fn apply_mutation_sqlite(
    connection: &mut SqliteConnection,
    write: &AdminUserMutationWrite,
    before: &AdminUser,
) -> Result<(), ApplicationError> {
    let id = uuid_blob(write.user_id);
    let now = sqlite_timestamp(write.now);
    match write.action {
        AdminUserMutationAction::Rename => {
            let username = write.username.as_deref().ok_or(AdminUserError::InvalidRequest)?;
            if before.username == username {
                return Err(admin_error(AdminUserError::Conflict));
            }
            sqlx::query(
                "INSERT INTO user_renames
                    (id, user_id, previous_username, next_username, actor, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(uuid_blob(write.command_id))
            .bind(&id)
            .bind(&before.username)
            .bind(username)
            .bind(&write.actor_id)
            .bind(&now)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_write_error)?;
            sqlx::query(
                "UPDATE users SET username = ?, state_version = state_version + 1,
                                  updated_at = ? WHERE id = ?",
            )
            .bind(username)
            .bind(&now)
            .bind(&id)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_write_error)?;
        }
        AdminUserMutationAction::SetGamePlayerId => {
            let game_player_id =
                write.game_player_id.as_deref().ok_or(AdminUserError::InvalidRequest)?;
            if before.game_player_id.as_deref() == Some(game_player_id) {
                return Err(admin_error(AdminUserError::Conflict));
            }
            sqlx::query(
                "UPDATE users SET game_player_id = ?, state_version = state_version + 1,
                                  updated_at = ? WHERE id = ?",
            )
            .bind(game_player_id)
            .bind(&now)
            .bind(&id)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_write_error)?;
        }
        AdminUserMutationAction::RevokeKey => {
            sqlx::query(
                "UPDATE users SET key_status = 'revoked', state_version = state_version + 1,
                                  updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&id)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_error)?;
        }
        AdminUserMutationAction::ReissueKey => {
            let key = write.key.as_ref().ok_or(AdminUserError::InvalidRequest)?;
            sqlx::query(
                "UPDATE users SET public_key_hash = ?, key_status = 'active',
                                  state_version = state_version + 1, updated_at = ? WHERE id = ?",
            )
            .bind(key.public_key_hash.to_vec())
            .bind(&now)
            .bind(&id)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_write_error)?;
            insert_envelope_sqlite(connection, write.user_id, &key.envelope, write.now).await?;
        }
        AdminUserMutationAction::Blacklist => {
            if before.status == AdminUserStatus::Blacklisted {
                return Err(admin_error(AdminUserError::Conflict));
            }
            let reason = write.public_reason.as_deref().ok_or(AdminUserError::InvalidRequest)?;
            sqlx::query(
                "UPDATE users SET status = 'blacklisted', state_version = state_version + 1,
                                  updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&id)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_error)?;
            sqlx::query(
                "INSERT INTO user_blacklist
                    (user_id, public_reason, actor, created_at, revoked_at)
                 VALUES (?, ?, ?, ?, NULL)
                 ON CONFLICT(user_id) DO UPDATE SET public_reason = excluded.public_reason,
                    actor = excluded.actor, created_at = excluded.created_at, revoked_at = NULL",
            )
            .bind(&id)
            .bind(reason)
            .bind(&write.actor_id)
            .bind(&now)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_write_error)?;
            sqlx::query(
                "INSERT INTO user_blacklist_history
                    (id, user_id, public_reason, actor, blacklisted_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(uuid_blob(write.command_id))
            .bind(&id)
            .bind(reason)
            .bind(&write.actor_id)
            .bind(&now)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_write_error)?;
        }
        AdminUserMutationAction::Unblacklist => {
            if before.status != AdminUserStatus::Blacklisted {
                return Err(admin_error(AdminUserError::Conflict));
            }
            sqlx::query(
                "UPDATE users SET status = 'active', state_version = state_version + 1,
                                  updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&id)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_error)?;
            sqlx::query(
                "UPDATE user_blacklist SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
            )
            .bind(&now)
            .bind(&id)
            .execute(&mut *connection)
            .await
            .map_err(map_sqlite_error)?;
        }
    }
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn revert_sqlite(
    store: &super::super::SqliteStore,
    write: AdminRenameRevertWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_revert_write(&write)?;
    let mut connection = begin_sqlite_write(store).await?;
    let result = revert_sqlite_inner(&mut connection, write).await;
    finish_sqlite_write(connection, result).await
}

#[cfg(feature = "sqlite")]
async fn revert_sqlite_inner(
    connection: &mut SqliteConnection,
    write: AdminRenameRevertWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    if let Some(result) = load_command_sqlite(
        connection,
        write.command_id,
        AdminUserCommandKind::RenameRevert,
        write.request_hash,
    )
    .await?
    {
        return Ok(result);
    }
    let rename = sqlx::query(
        "SELECT user_id, previous_username, next_username, reverted_at
         FROM user_renames WHERE id = ?",
    )
    .bind(uuid_blob(write.rename_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlite_error)?
    .ok_or(AdminUserError::NotFound)?;
    let user_id = uuid_from_blob(rename.try_get("user_id").map_err(map_sqlite_error)?)?;
    let previous_username: String =
        rename.try_get("previous_username").map_err(map_sqlite_error)?;
    let next_username: String = rename.try_get("next_username").map_err(map_sqlite_error)?;
    let reverted_at: Option<String> = rename.try_get("reverted_at").map_err(map_sqlite_error)?;
    if reverted_at.is_some() {
        return Err(admin_error(AdminUserError::RenameNotRevertable));
    }
    let before = load_user_sqlite(connection, user_id).await?;
    if before.state_version != write.expected_state_version {
        return Err(admin_error(AdminUserError::VersionConflict {
            current_version: before.state_version,
        }));
    }
    if before.username != next_username {
        return Err(admin_error(AdminUserError::RenameNotRevertable));
    }
    let now = sqlite_timestamp(write.now);
    sqlx::query(
        "UPDATE users SET username = ?, state_version = state_version + 1,
                          updated_at = ? WHERE id = ?",
    )
    .bind(&previous_username)
    .bind(&now)
    .bind(uuid_blob(user_id))
    .execute(&mut *connection)
    .await
    .map_err(map_sqlite_write_error)?;
    sqlx::query("UPDATE user_renames SET reverted_at = ? WHERE id = ?")
        .bind(&now)
        .bind(uuid_blob(write.rename_id))
        .execute(&mut *connection)
        .await
        .map_err(map_sqlite_error)?;
    let result = AdminUserCommandResult {
        user: load_user_sqlite(connection, user_id).await?,
        response_envelope: None,
    };
    insert_command_sqlite(
        connection,
        write.command_id,
        AdminUserCommandKind::RenameRevert,
        write.request_hash,
        &result,
        user_id,
        write.now,
    )
    .await?;
    insert_audit_sqlite(
        connection,
        write.command_id,
        AdminUserCommandKind::RenameRevert,
        user_id,
        &write.actor_id,
        write.request_id.as_deref(),
        write.now,
    )
    .await?;
    Ok(result)
}

#[cfg(feature = "sqlite")]
async fn load_user_sqlite(
    connection: &mut SqliteConnection,
    user_id: Uuid,
) -> Result<AdminUser, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, username, game_player_id, status, key_status, state_version,
                created_at, updated_at FROM users WHERE id = ?",
    )
    .bind(uuid_blob(user_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlite_error)?
    .ok_or(AdminUserError::NotFound)?;
    map_sqlite_user(&row)
}

#[cfg(feature = "sqlite")]
fn map_sqlite_user(row: &SqliteRow) -> Result<AdminUser, ApplicationError> {
    let status: String = row.try_get("status").map_err(map_sqlite_error)?;
    let key_status: String = row.try_get("key_status").map_err(map_sqlite_error)?;
    let user = AdminUser {
        id: uuid_from_blob(row.try_get("id").map_err(map_sqlite_error)?)?,
        username: row.try_get("username").map_err(map_sqlite_error)?,
        game_player_id: row.try_get("game_player_id").map_err(map_sqlite_error)?,
        status: AdminUserStatus::parse(&status)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?,
        key_status: auction_application::AdminUserKeyStatus::parse(&key_status)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?,
        state_version: row.try_get("state_version").map_err(map_sqlite_error)?,
        created_at: parse_sqlite_timestamp(
            row.try_get::<String, _>("created_at").map_err(map_sqlite_error)?,
        )?,
        updated_at: parse_sqlite_timestamp(
            row.try_get::<String, _>("updated_at").map_err(map_sqlite_error)?,
        )?,
    };
    user.validate().then_some(user).ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))
}

#[cfg(feature = "sqlite")]
async fn load_command_sqlite(
    connection: &mut SqliteConnection,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
) -> Result<Option<AdminUserCommandResult>, ApplicationError> {
    let row = sqlx::query(
        "SELECT command_kind, request_digest, user_id, result_payload,
                response_ciphertext, response_nonce, response_encryption_key_id,
                response_fingerprint
         FROM user_admin_commands WHERE command_id = ?",
    )
    .bind(uuid_blob(command_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    row.map(|row| parse_sqlite_command(&row, kind, request_hash)).transpose()
}

#[cfg(feature = "sqlite")]
fn parse_sqlite_command(
    row: &SqliteRow,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
) -> Result<AdminUserCommandResult, ApplicationError> {
    let stored_kind: String = row.try_get("command_kind").map_err(map_sqlite_error)?;
    let stored_hash: Vec<u8> = row.try_get("request_digest").map_err(map_sqlite_error)?;
    if stored_kind != kind.as_str() || stored_hash.as_slice() != request_hash.as_bytes() {
        return Err(admin_error(AdminUserError::CommandConflict));
    }
    let user_id = uuid_from_blob(row.try_get("user_id").map_err(map_sqlite_error)?)?;
    let payload: String = row.try_get("result_payload").map_err(map_sqlite_error)?;
    let payload = serde_json::from_str(&payload)
        .map_err(|_| admin_error(AdminUserError::StoredResultCorrupt))?;
    let response = load_response_envelope(
        row.try_get("response_ciphertext").map_err(map_sqlite_error)?,
        row.try_get("response_nonce").map_err(map_sqlite_error)?,
        row.try_get("response_encryption_key_id").map_err(map_sqlite_error)?,
        row.try_get("response_fingerprint").map_err(map_sqlite_error)?,
    )?;
    parse_payload(payload, user_id, response)
}

#[cfg(feature = "sqlite")]
async fn insert_envelope_sqlite(
    connection: &mut SqliteConnection,
    user_id: Uuid,
    envelope: &MailLoginKeyEnvelope,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO user_login_key_envelopes
            (user_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET ciphertext = excluded.ciphertext,
             nonce = excluded.nonce, encryption_key_id = excluded.encryption_key_id,
             fingerprint = excluded.fingerprint, updated_at = excluded.updated_at",
    )
    .bind(uuid_blob(user_id))
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(envelope.fingerprint())
    .bind(sqlite_timestamp(now))
    .bind(sqlite_timestamp(now))
    .execute(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_command_sqlite(
    connection: &mut SqliteConnection,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    request_hash: RequestHash,
    result: &AdminUserCommandResult,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let response = result.response_envelope.as_ref();
    let payload = serde_json::to_string(&json!({ "user": &result.user }))?;
    sqlx::query(
        "INSERT INTO user_admin_commands
            (command_id, command_kind, request_digest, user_id, result_payload,
             response_ciphertext, response_nonce, response_encryption_key_id,
             response_fingerprint, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_blob(command_id))
    .bind(kind.as_str())
    .bind(request_hash.as_bytes().to_vec())
    .bind(uuid_blob(user_id))
    .bind(payload)
    .bind(response.map(|value| value.ciphertext().to_vec()))
    .bind(response.map(|value| value.nonce().to_vec()))
    .bind(response.map(|value| value.encryption_key_id().to_owned()))
    .bind(response.map(|value| value.fingerprint().to_owned()))
    .bind(sqlite_timestamp(now))
    .execute(&mut *connection)
    .await
    .map_err(map_sqlite_error)?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn insert_audit_sqlite(
    connection: &mut SqliteConnection,
    command_id: Uuid,
    kind: AdminUserCommandKind,
    user_id: Uuid,
    actor_id: &str,
    request_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let metadata =
        serde_json::to_string(&json!({ "command_id": command_id, "action": kind.as_str() }))?;
    sqlx::query(
        "INSERT INTO audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, request_id, source, metadata)
         VALUES (?, ?, 'admin', ?, ?, 'user', ?, 'success', ?, 'admin', ?)",
    )
    .bind(uuid_blob(Uuid::new_v4()))
    .bind(sqlite_timestamp(now))
    .bind(actor_id)
    .bind(format!("user.{}", kind.as_str()))
    .bind(user_id.to_string())
    .bind(request_id)
    .bind(metadata)
    .execute(&mut *connection)
    .await
    .record_audit_failure()
    .map_err(map_sqlite_error)?;
    Ok(())
}

#[cfg(feature = "sqlite")]
async fn begin_sqlite_write(
    store: &super::super::SqliteStore,
) -> Result<PoolConnection<sqlx::Sqlite>, ApplicationError> {
    let mut connection = store.pool().acquire().await.map_err(map_sqlite_error)?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await.map_err(map_sqlite_error)?;
    Ok(connection)
}

#[cfg(feature = "sqlite")]
async fn finish_sqlite_write<T>(
    mut connection: PoolConnection<sqlx::Sqlite>,
    result: Result<T, ApplicationError>,
) -> Result<T, ApplicationError> {
    match result {
        Ok(value) => {
            match sqlx::query("COMMIT").execute(&mut *connection).await {
                Ok(_) => Ok(value),
                Err(error) => {
                    // 提交失败时尽力回滚，避免连接回收到连接池时仍保留未完成事务。
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    Err(map_sqlite_error(error))
                }
            }
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

#[cfg(feature = "sqlite")]
fn uuid_blob(value: Uuid) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(feature = "sqlite")]
fn uuid_from_blob(value: Vec<u8>) -> Result<Uuid, ApplicationError> {
    Uuid::from_slice(&value).map_err(|_| storage_error("Lite Admin 用户 UUID 格式无效"))
}

#[cfg(feature = "sqlite")]
fn sqlite_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

#[cfg(feature = "sqlite")]
fn parse_sqlite_timestamp(value: String) -> Result<DateTime<Utc>, ApplicationError> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&value) {
        return Ok(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(&value, format) {
            return Ok(DateTime::from_naive_utc_and_offset(parsed, Utc));
        }
    }
    Err(storage_error("Lite Admin 用户时间格式无效"))
}

#[cfg(feature = "sqlite")]
fn map_sqlite_write_error(error: sqlx::Error) -> ApplicationError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.message().contains("UNIQUE")
                || database_error.message().contains("unique") =>
        {
            admin_error(AdminUserError::Conflict)
        }
        _ => map_sqlite_error(error),
    }
}

#[cfg(feature = "sqlite")]
fn map_sqlite_error(error: sqlx::Error) -> ApplicationError {
    let _ = error;
    storage_error("Lite Admin Users 存储操作失败")
}
