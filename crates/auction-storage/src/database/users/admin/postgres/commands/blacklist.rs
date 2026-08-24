//! PostgreSQL recovery buyer 黑名单命令。

use super::super::mapping::*;
use super::super::reads::{load_command_postgres_tx, load_user_postgres_tx};
use super::super::*;

pub(crate) async fn ensure_recovery_blacklist_postgres(
    store: &super::super::super::super::super::PostgresStore,
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
