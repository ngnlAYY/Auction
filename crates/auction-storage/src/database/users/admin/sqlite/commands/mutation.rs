//! SQLite Users/Admin 用户属性、密钥与黑名单命令。

use super::blacklist::apply_blacklist_effects_sqlite;
use super::*;

pub(crate) async fn mutate_sqlite(
    store: &crate::database::SqliteStore,
    write: AdminUserMutationWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_mutation_write(&write)?;
    let mut connection = begin_sqlite_write(store).await?;
    let result = mutate_sqlite_inner(&mut connection, write).await;
    finish_sqlite_write(connection, result).await
}

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
    if write.action == AdminUserMutationAction::Blacklist {
        apply_blacklist_effects_sqlite(connection, write.command_id, write.user_id, write.now)
            .await?;
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
