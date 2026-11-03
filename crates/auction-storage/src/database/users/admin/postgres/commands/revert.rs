//! PostgreSQL Admin Users 改名回滚命令。

use super::super::mapping::*;
use super::super::reads::{load_command_postgres_tx, load_user_postgres_tx};
use super::super::*;

pub(crate) async fn revert_postgres(
    store: &super::super::super::super::super::PostgresStore,
    write: AdminRenameRevertWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    store
        .pool()
        .observe_transaction(auction_observability::DatabaseTransactionOwner::Admin, async {
            validate_revert_write(&write)?;
            let mut transaction = store
                .pool()
                .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                .await
                .map_err(map_storage_error)?;
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
            let next_username: String =
                rename.try_get("next_username").map_err(map_storage_error)?;
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
         SET username = $1,
             game_player_id = $1,
             state_version = state_version + 1,
             updated_at = GREATEST(created_at, updated_at, $2)
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
        })
        .await
}
