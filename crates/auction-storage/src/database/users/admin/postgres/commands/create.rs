//! PostgreSQL Admin Users 创建命令。

use super::super::mapping::*;
use super::super::reads::{load_command_postgres_tx, load_user_postgres_tx};
use crate::database::users::admin::validate_create_write;
use crate::database::users::envelope::upsert_login_key_envelope;
use auction_application::{
    AdminUserCommandKind, AdminUserCommandResult, AdminUserCreateWrite, ApplicationError,
};

pub(crate) async fn create_postgres(
    store: &super::super::super::super::super::PostgresStore,
    write: AdminUserCreateWrite,
) -> Result<AdminUserCommandResult, ApplicationError> {
    store
        .pool()
        .observe_transaction(auction_observability::DatabaseTransactionOwner::Admin, async {
            validate_create_write(&write)?;
            let mut transaction = store
                .pool()
                .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
                .await
                .map_err(map_storage_error)?;
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
            (id, username, status, public_key_hash, game_player_id, forum_account_id,
             key_status, state_version, created_at, updated_at)
         VALUES ($1, $2, 'active', $3, $2, $4, 'active', 1, $5, $5)",
            )
            .bind(write.user_id)
            .bind(&write.username)
            .bind(write.key.public_key_hash.as_slice())
            .bind(write.forum_account_id.as_deref())
            .bind(write.now)
            .execute(&mut *transaction)
            .await
            .map_err(map_admin_write_error)?;
            upsert_login_key_envelope(
                &mut transaction,
                write.user_id,
                &write.key.envelope,
                write.now,
            )
            .await
            .map_err(map_storage_error)?;
            let user = load_user_postgres_tx(&mut transaction, write.user_id).await?;
            let result =
                AdminUserCommandResult { user, response_envelope: write.key.response_envelope };
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
        })
        .await
}
