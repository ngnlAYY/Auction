//! SQLite Users/Admin 用户创建命令。

use super::*;

pub(crate) async fn create_sqlite(
    store: &crate::database::SqliteStore,
    write: AdminUserCreateWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    validate_create_write(&write)?;
    let mut connection = begin_sqlite_write(store).await?;
    let result = create_sqlite_inner(&mut connection, write).await;
    finish_sqlite_write(connection, result).await
}

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
            (id, username, status, public_key_hash, game_player_id, forum_account_id,
             key_status, state_version, created_at, updated_at)
         VALUES (?, ?, 'active', ?, ?, ?, 'active', 1, ?, ?)",
    )
    .bind(uuid_blob(write.user_id))
    .bind(&write.username)
    .bind(write.key.public_key_hash.to_vec())
    .bind(&write.username)
    .bind(write.forum_account_id.as_deref())
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
