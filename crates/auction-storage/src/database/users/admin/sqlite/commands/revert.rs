//! SQLite Users/Admin 改名回滚命令。

use super::*;

pub(crate) async fn revert_sqlite(
    store: &crate::database::SqliteStore,
    write: AdminRenameRevertWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_revert_write(&write)?;
    let mut connection = begin_sqlite_write(store).await?;
    let result = revert_sqlite_inner(&mut connection, write).await;
    finish_sqlite_write(connection, result).await
}

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
        "UPDATE users SET username = ?, game_player_id = ?,
                          state_version = state_version + 1,
                          updated_at = ? WHERE id = ?",
    )
    .bind(&previous_username)
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
