//! PostgreSQL Admin Users 状态/身份变更命令。

use super::super::mapping::*;
use super::super::reads::{load_command_postgres_tx, load_user_postgres_tx};
use super::super::*;
use super::blacklist::apply_blacklist_effects_postgres;

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

    // 竞价事务使用 lot -> proxy -> user 的锁序；拉黑必须先取得相同的 lot/proxy 锁，
    // 才能在用户资格变更前原子冻结授权，避免与并发出价形成反向锁。
    if write.action == AdminUserMutationAction::Blacklist {
        apply_blacklist_effects_postgres(
            &mut transaction,
            write.command_id,
            write.user_id,
            write.now,
        )
        .await?;
    }
    if write.action == AdminUserMutationAction::SetForumAccountId {
        let forum_account_id =
            write.forum_account_id.as_deref().ok_or(AdminUserError::InvalidRequest)?;
        lock_forum_identity_ownership_postgres(&mut transaction, write.user_id, forum_account_id)
            .await?;
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
                 SET username = $1, game_player_id = $1,
                     state_version = state_version + 1, updated_at = $2
                 WHERE id = $3",
            )
            .bind(username)
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_admin_write_error)?;
        }
        AdminUserMutationAction::SetForumAccountId => {
            let forum_account_id =
                write.forum_account_id.as_deref().ok_or(AdminUserError::InvalidRequest)?;
            if before.forum_account_id.as_deref() == Some(forum_account_id) {
                return Err(admin_error(AdminUserError::Conflict));
            }
            // showuser 是跨世界稳定账号键；Admin 修正时必须与所有已证明论坛映射同事务更新，
            // 否则下一轮回帖会把 Users 与映射表的分叉当作身份劫持并永久拒绝。
            sqlx::query(
                "UPDATE public.forum_bidder_identities
                 SET external_account_id = $1, updated_at = $2
                 WHERE user_id = $3 AND external_account_id IS DISTINCT FROM $1",
            )
            .bind(forum_account_id)
            .bind(now)
            .bind(id)
            .execute(&mut **transaction)
            .await
            .map_err(map_admin_write_error)?;
            sqlx::query(
                "UPDATE public.users
                 SET forum_account_id = $1, state_version = state_version + 1, updated_at = $2
                 WHERE id = $3",
            )
            .bind(forum_account_id)
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

async fn lock_forum_identity_ownership_postgres(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    forum_account_id: &str,
) -> Result<(), ApplicationError> {
    // 与论坛 resolver 共用 showuser advisory lock，并先锁映射再锁 Users 行，保持相同锁序。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(forum_account_id)
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    sqlx::query(
        "SELECT id
         FROM public.forum_bidder_identities
         WHERE user_id = $1
         ORDER BY world_id, provider
         FOR UPDATE",
    )
    .bind(user_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    Ok(())
}
