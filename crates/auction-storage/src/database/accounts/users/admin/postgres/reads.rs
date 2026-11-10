//! PostgreSQL Users/Admin 查询、列表和幂等命令读取。

use std::collections::HashMap;

use futures_util::TryStreamExt;

use super::mapping::{map_postgres_user, map_storage_error, parse_postgres_command};
use crate::database::accounts::users::admin::admin_error;
use auction_application::admin::{
    AdminUser, AdminUserBlacklistHistoryEntry, AdminUserCommandKind, AdminUserCommandResult,
    AdminUserError, AdminUserListItem, AdminUserListRequest, AdminUserPage, AdminUserStatus,
};
use auction_application::core::{ApplicationError, RequestHash};
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn list_postgres(
    store: &crate::database::DatabaseStore,
    request: AdminUserListRequest,
) -> Result<AdminUserPage, ApplicationError> {
    store
        .pool()
        .observe_transaction(auction_observability::DatabaseTransactionOwner::Admin, async {
            let mut transaction = store
                .pool()
                .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                .await
                .map_err(map_storage_error)?;
            let name = request.name.as_deref().filter(|value| !value.is_empty());
            let status = request.status.map(AdminUserStatus::as_str);
            let total: i64 = sqlx::query_scalar(
                "SELECT count(*)
         FROM public.users
         WHERE ($1::text IS NULL OR strpos(username, $1) > 0
                OR strpos(forum_account_id, $1) > 0)
           AND ($2::text IS NULL OR status = $2)",
            )
            .bind(name)
            .bind(status)
            .fetch_one(&mut *transaction)
            .await
            .map_err(map_storage_error)?;
            let rows = sqlx::query(
                "SELECT id, username, forum_account_id, status, key_status, state_version,
                created_at, updated_at
         FROM public.users
         WHERE ($1::text IS NULL OR strpos(username, $1) > 0
                OR strpos(forum_account_id, $1) > 0)
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
            let page_users = rows
                .into_iter()
                .map(|row| map_postgres_user(&row))
                .collect::<Result<Vec<_>, _>>()?;
            let user_ids = page_users.iter().map(|user| user.id).collect::<Vec<_>>();
            let mut histories =
                load_blacklist_histories_postgres(&mut transaction, &user_ids).await?;
            let users = page_users
                .into_iter()
                .map(|user| {
                    let (blacklist_count, blacklist_history) =
                        histories.remove(&user.id).unwrap_or_default();
                    AdminUserListItem { user, blacklist_count, blacklist_history }
                })
                .collect();
            transaction.commit().await.map_err(map_storage_error)?;
            let page = AdminUserPage { users, total, limit: request.limit, offset: request.offset };
            if page.validate(&request) {
                Ok(page)
            } else {
                Err(admin_error(AdminUserError::StoredResultCorrupt))
            }
        })
        .await
}

async fn load_blacklist_histories_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, (i64, Vec<AdminUserBlacklistHistoryEntry>)>, ApplicationError> {
    let mut histories = HashMap::<Uuid, (i64, Vec<AdminUserBlacklistHistoryEntry>)>::new();
    if user_ids.is_empty() {
        return Ok(histories);
    }
    // 每个当前页用户仍返回最近十条和完整计数，只合并数据库往返，不改变历史截取边界。
    let mut rows = sqlx::query(
        "SELECT selected.user_id, history.public_reason, history.blacklisted_at,
                history.total_count
         FROM unnest($1::uuid[]) AS selected(user_id)
         CROSS JOIN LATERAL (
             SELECT id, public_reason, blacklisted_at, count(*) OVER () AS total_count
             FROM public.user_blacklist_history
             WHERE user_id = selected.user_id
             ORDER BY blacklisted_at DESC, id DESC
             LIMIT 10
         ) AS history
         ORDER BY selected.user_id, history.blacklisted_at DESC, history.id DESC",
    )
    .bind(user_ids)
    .fetch(&mut **transaction);
    while let Some(row) = rows.try_next().await.map_err(map_storage_error)? {
        let user_id = row.try_get("user_id").map_err(map_storage_error)?;
        let total_count = row.try_get("total_count").map_err(map_storage_error)?;
        let (_, history) = histories.entry(user_id).or_insert_with(|| (total_count, Vec::new()));
        history.push(AdminUserBlacklistHistoryEntry {
            public_reason: row.try_get("public_reason").map_err(map_storage_error)?,
            blacklisted_at: row.try_get("blacklisted_at").map_err(map_storage_error)?,
        });
    }
    Ok(histories)
}

pub(crate) async fn find_command_postgres(
    store: &crate::database::DatabaseStore,
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
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<AdminUser, ApplicationError> {
    let row = sqlx::query(
        "SELECT id, username, forum_account_id, status, key_status, state_version,
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
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
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
