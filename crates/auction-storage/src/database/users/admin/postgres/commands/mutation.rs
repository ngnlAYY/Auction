//! PostgreSQL Admin Users 状态/身份变更命令。

use super::super::mapping::*;
use super::super::reads::{load_command_postgres_tx, load_user_postgres_tx};
use super::super::*;

pub(crate) async fn mutate_postgres(
    store: &super::super::super::super::super::PostgresStore,
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
                 SET public_reason = EXCLUDED.public_reason, actor = EXCLUDED.actor, created_at = EXCLUDED.created_at,
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
