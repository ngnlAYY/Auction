//! SQLite Lite Users/Admin 查询、列表和幂等命令读取。

use super::mapping::{
    map_sqlite_error, map_sqlite_user, parse_sqlite_command, parse_sqlite_timestamp, uuid_blob,
};
use super::*;

pub(crate) async fn list_sqlite(
    store: &crate::database::SqliteStore,
    request: AdminUserListRequest,
) -> Result<AdminUserPage, ApplicationError> {
    let mut transaction = store.pool().begin().await.map_err(map_sqlite_error)?;
    let name = request.name.as_deref().filter(|value| !value.is_empty());
    let status = request.status.map(AdminUserStatus::as_str);
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM users
         WHERE (? IS NULL OR instr(username, ?) > 0
                OR instr(forum_account_id, ?) > 0)
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
        "SELECT id, username, forum_account_id, status, key_status, state_version,
                created_at, updated_at
         FROM users
         WHERE (? IS NULL OR instr(username, ?) > 0
                OR instr(forum_account_id, ?) > 0)
           AND (? IS NULL OR status = ?)
         ORDER BY username COLLATE BINARY, id
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

pub(crate) async fn find_command_sqlite(
    store: &crate::database::SqliteStore,
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

pub(super) async fn load_user_sqlite(
    connection: &mut SqliteConnection,
    user_id: Uuid,
) -> Result<AdminUser, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, username, forum_account_id, status, key_status, state_version,
                created_at, updated_at FROM users WHERE id = ?",
    )
    .bind(uuid_blob(user_id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(map_sqlite_error)?
    .ok_or(AdminUserError::NotFound)?;
    map_sqlite_user(&row)
}

pub(super) async fn load_command_sqlite(
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
