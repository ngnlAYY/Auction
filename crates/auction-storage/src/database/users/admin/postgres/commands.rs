//! PostgreSQL Users/Admin 命令写入与事务状态变更。

use super::mapping::*;
use super::reads::{load_command_postgres_tx, load_user_postgres_tx};
use super::*;

pub(crate) async fn ensure_recovery_blacklist_postgres(
    store: &super::super::super::super::PostgresStore,
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

pub(crate) async fn create_postgres(
    store: &super::super::super::super::PostgresStore,
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

pub(crate) async fn mutate_postgres(
    store: &super::super::super::super::PostgresStore,
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

pub(crate) async fn revert_postgres(
    store: &super::super::super::super::PostgresStore,
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
