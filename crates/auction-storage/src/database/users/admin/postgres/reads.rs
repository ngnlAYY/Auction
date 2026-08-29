//! PostgreSQL Users/Admin 查询、列表和幂等命令读取。

use super::mapping::{map_postgres_user, map_storage_error, parse_postgres_command};
use super::*;

pub(crate) async fn list_postgres(
    store: &super::super::super::super::PostgresStore,
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
         WHERE ($1::text IS NULL OR strpos(username, $1) > 0
                OR strpos(game_player_id, $1) > 0)
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
         WHERE ($1::text IS NULL OR strpos(username, $1) > 0
                OR strpos(game_player_id, $1) > 0)
           AND ($2::text IS NULL OR status = $2)
         ORDER BY username COLLATE \"C\", id
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

pub(crate) async fn find_command_postgres(
    store: &super::super::super::super::PostgresStore,
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

pub(super) async fn load_user_postgres_tx(
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

pub(super) async fn load_command_postgres_tx(
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
