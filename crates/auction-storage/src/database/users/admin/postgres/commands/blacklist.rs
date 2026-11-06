//! PostgreSQL recovery buyer 黑名单命令。

use auction_application::core::{TransactionFailure, TransactionPolicy};

use super::super::mapping::*;
use super::super::reads::{load_command_postgres_tx, load_user_postgres_tx};
use crate::database::users::admin::admin_error;
use auction_application::admin::{
    AdminUser, AdminUserCommandKind, AdminUserCommandResult, AdminUserError, AdminUserStatus,
    RECOVERY_BUYER_BLACKLIST_ACTOR, RecoveryBuyerBlacklistCommand, RecoveryBuyerBlacklistResult,
};
use auction_application::core::{ApplicationError, RequestHash};
use auction_domain::{CommandId, deterministic_uuid};
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::Row;
use sqlx::types::Json;
use uuid::Uuid;

pub(crate) async fn ensure_recovery_blacklist_postgres(
    store: &super::super::super::super::super::PostgresStore,
    command: RecoveryBuyerBlacklistCommand,
) -> Result<RecoveryBuyerBlacklistResult, ApplicationError> {
    // 与竞价事务共用三次以内的有界策略；每次重试仍借用同一命令、摘要和历史时间。
    crate::database::retry_conflicts(
        store.pool(),
        auction_observability::DatabaseTransactionOwner::Admin,
        usize::from(TransactionPolicy::serializable().max_attempts),
        ApplicationError::is_serialization_conflict,
        || ensure_recovery_blacklist_postgres_once(store, &command),
    )
    .await
}

async fn ensure_recovery_blacklist_postgres_once(
    store: &super::super::super::super::super::PostgresStore,
    command: &RecoveryBuyerBlacklistCommand,
) -> Result<RecoveryBuyerBlacklistResult, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE")
        .await
        .map_err(map_storage_error)?;
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
        let result = recovery_blacklist_result(command, result.user)?;
        transaction.commit().await.map_err(map_storage_error)?;
        return Ok(result);
    }

    // recovery 与人工拉黑共用完整副作用；即使用户已是 blacklisted，也要修复仍存活的
    // 代理授权和公开会话，再为 recovery 命令保存独立 proof。
    freeze_blacklist_proxies_postgres(
        &mut transaction,
        command.command_id,
        command.buyer_user_id,
        command.occurred_at,
    )
    .await?;
    let before = load_user_postgres_tx(&mut transaction, command.buyer_user_id).await?;
    // recovery 时间来自历史 claim；快照副作用不能早于用户已有状态，账本/审计仍保留原命令时间。
    let side_effect_at = command.occurred_at.max(before.created_at).max(before.updated_at);
    if before.status == AdminUserStatus::Active {
        sqlx::query(
            "UPDATE public.users
             SET status = 'blacklisted', state_version = state_version + 1,
                 updated_at = GREATEST(created_at, updated_at, $1)
             WHERE id = $2",
        )
        .bind(side_effect_at)
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
        .bind(&command.public_reason)
        .bind(RECOVERY_BUYER_BLACKLIST_ACTOR)
        .bind(side_effect_at)
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
        .bind(&command.public_reason)
        .bind(RECOVERY_BUYER_BLACKLIST_ACTOR)
        .bind(side_effect_at)
        .execute(&mut *transaction)
        .await
        .map_err(map_admin_write_error)?;
    }
    revoke_public_sessions_postgres(&mut transaction, command.buyer_user_id, side_effect_at)
        .await?;

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
    let result = recovery_blacklist_result(command, user)?;
    transaction.commit().await.map_err(map_storage_error)?;
    Ok(result)
}

/// 在 Users 拉黑事务内冻结有效代理并清理慢速调度；这里不锁公开会话。
/// 锁顺序固定为 lot -> proxy，调用方随后必须锁 user，再撤销 session，与竞价和登录事务一致。
/// 每个代理转换写入确定性拍卖子命令，使追加式转换的 `origin_command_id` 始终有账本证据。
pub(super) async fn freeze_blacklist_proxies_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    source_command_id: Uuid,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let locked_lots = sqlx::query(
        "SELECT lot.id
         FROM public.lots AS lot
         WHERE EXISTS (
             SELECT 1
             FROM public.bid_proxy_authorizations AS proxy
             WHERE proxy.lot_id = lot.id
               AND proxy.user_id = $1
               AND proxy.status IN ('active', 'final_fast')
         )
         ORDER BY lot.id
         FOR UPDATE OF lot",
    )
    .bind(user_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;

    let proxies = sqlx::query(
        "SELECT proxy.authorization_id, proxy.lot_id, proxy.maximum_amount,
                proxy.mode, proxy.status, proxy.state_version,
                GREATEST(proxy.created_at, proxy.updated_at, (
                    SELECT max(transition.created_at)
                    FROM public.bid_proxy_authorization_transitions AS transition
                    WHERE transition.authorization_id = proxy.authorization_id
                )) AS last_transition_at
         FROM public.bid_proxy_authorizations AS proxy
         WHERE proxy.user_id = $1 AND proxy.status IN ('active', 'final_fast')
         ORDER BY proxy.lot_id, proxy.user_id, proxy.authorization_id
         FOR UPDATE OF proxy",
    )
    .bind(user_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    if locked_lots.len() != proxies.len() {
        return Err(admin_error(AdminUserError::StoredResultCorrupt));
    }

    for row in proxies {
        let authorization_id: Uuid = row.try_get("authorization_id").map_err(map_storage_error)?;
        let lot_id: Uuid = row.try_get("lot_id").map_err(map_storage_error)?;
        let maximum_amount: i64 = row.try_get("maximum_amount").map_err(map_storage_error)?;
        let mode: String = row.try_get("mode").map_err(map_storage_error)?;
        let status: String = row.try_get("status").map_err(map_storage_error)?;
        let state_version: i64 = row.try_get("state_version").map_err(map_storage_error)?;
        let last_transition_at: DateTime<Utc> =
            row.try_get("last_transition_at").map_err(map_storage_error)?;
        // 新授权可能晚于历史 recovery claim；冻结快照和追加转换共用单调副作用时间，
        // 确定性子命令仍使用原 now，不能改变命令重放身份或历史证据。
        let side_effect_at = now.max(last_transition_at);
        let next_version = state_version
            .checked_add(1)
            .filter(|version| maximum_amount > 0 && *version > 1)
            .ok_or_else(|| admin_error(AdminUserError::StoredResultCorrupt))?;
        if !matches!(mode.as_str(), "fast" | "slow")
            || !matches!(status.as_str(), "active" | "final_fast")
        {
            return Err(admin_error(AdminUserError::StoredResultCorrupt));
        }

        let child_command_id = deterministic_uuid(
            CommandId::from_uuid(source_command_id),
            &format!("users-freeze-proxy-authorization:{authorization_id}"),
            1,
        );
        let request = json!({
            "authorization_id": authorization_id,
            "source_command_id": source_command_id,
            "reason": "eligibility_lost",
        });
        let request_hash =
            RequestHash::from_request("freeze_proxy_authorization", lot_id, &request)?;
        let result_payload = json!({
            "authorization_id": authorization_id,
            "source_command_id": source_command_id,
            "status": "frozen",
        });
        sqlx::query(
            "INSERT INTO public.auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code,
                 result_payload, created_at)
             VALUES ($1, 'freeze_proxy_authorization', $2, $3, 'accepted', $4, $5)",
        )
        .bind(child_command_id)
        .bind(lot_id)
        .bind(request_hash.as_bytes().as_slice())
        .bind(Json(result_payload))
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(map_admin_write_error)?;

        let updated = sqlx::query(
            "UPDATE public.bid_proxy_authorizations
             SET status = 'frozen', terminal_reason = 'eligibility_lost',
                 state_version = $2, updated_at = $3
             WHERE authorization_id = $1 AND status = $4 AND state_version = $5",
        )
        .bind(authorization_id)
        .bind(next_version)
        .bind(side_effect_at)
        .bind(&status)
        .bind(state_version)
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(admin_error(AdminUserError::StoredResultCorrupt));
        }

        sqlx::query(
            "INSERT INTO public.bid_proxy_authorization_transitions
                (authorization_id, transition_number, transition_kind, origin_command_id,
                 old_maximum_amount, new_maximum_amount, old_mode, new_mode, old_status,
                 new_status, reason, resulting_state_version, created_at)
             VALUES ($1, $2, 'frozen', $3, $4, $4, $5, $5, $6,
                     'frozen', 'eligibility_lost', $2, $7)",
        )
        .bind(authorization_id)
        .bind(next_version)
        .bind(child_command_id)
        .bind(maximum_amount)
        .bind(&mode)
        .bind(&status)
        .bind(side_effect_at)
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;

        sqlx::query(
            "UPDATE public.lots
             SET slow_authorization_id = NULL, slow_due_at = NULL,
                 slow_generation = slow_generation + 1,
                 updated_at = GREATEST(created_at, updated_at, $2)
             WHERE id = $1 AND slow_authorization_id = $3",
        )
        .bind(lot_id)
        .bind(side_effect_at)
        .bind(authorization_id)
        .execute(&mut **transaction)
        .await
        .map_err(map_storage_error)?;
    }
    Ok(())
}

/// 调用方须已持有 user 行锁；会话撤销不能早于用户锁，避免与登录或偏好写入反向等待。
pub(super) async fn revoke_public_sessions_postgres(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "UPDATE public.public_sessions
         SET revoked_at = GREATEST(created_at, $1)
         WHERE user_id = $2 AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(user_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    Ok(())
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
