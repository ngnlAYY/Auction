//! Recovery action 从 prepared 激活为 enqueued 的 PostgreSQL 写入。

use {
    crate::database::recovery::scheduler::actions::{RecoveryActionTransactionError, lock_action},
    crate::database::recovery::scheduler::{map_storage_error, repository_error},
    auction_application::core::RequestHash,
    auction_application::recovery::RecoveryActionClaim,
    auction_domain::CommandId,
    chrono::{DateTime, Utc},
    sqlx::{Postgres, Row},
    uuid::Uuid,
};

pub(crate) async fn mark_action_enqueued_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &RecoveryActionClaim,
    activate_command_id: CommandId,
    activate_request_hash: RequestHash,
    _activated_at: DateTime<Utc>,
) -> Result<RecoveryActionClaim, auction_application::core::RepositoryError> {
    let delivery_id = claim
        .delivery_id
        .ok_or_else(|| repository_error("recovery prepared action 缺少 delivery 标识"))?;
    let prepare_command_id = claim
        .prepare_command_id
        .ok_or_else(|| repository_error("recovery prepared action 缺少 prepare command"))?;
    let prepare_request_hash = claim
        .prepare_request_hash
        .ok_or_else(|| repository_error("recovery prepared action 缺少 prepare digest"))?;
    let state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error("recovery action 状态版本溢出"))?;
    // 重试只捕获固定身份与 fence，避免复制收件人、附件和诊断等完整 claim。
    let action_id = claim.action_id;
    let recovery_id = claim.recovery_id;
    let expected_state_version = claim.state_version;
    let claim_token = claim.claim_token;
    let fencing_version = claim.fencing_version;
    let activate_digest = activate_request_hash.as_bytes();
    let prepare_digest = prepare_request_hash.as_bytes();
    super::super::run_serializable_action_transaction(pool, move |transaction| {
        Box::pin(async move {
            lock_action(transaction, action_id.as_uuid()).await?;
            let result = sqlx::query(
                r#"UPDATE public.settlement_recovery_actions
                   SET execution_state = 'enqueued',
                       activate_command_id = $1,
                       activate_request_digest = $2,
                       last_error_code = NULL,
                       next_attempt_at = NULL,
                       state_version = state_version + 1,
                       updated_at = clock_timestamp()
                   WHERE id = $3
                     AND recovery_id = $4
                     AND execution_state = 'prepared'
                     AND state_version = $5
                     AND claim_token = $6
                     AND claim_fencing = $7
                     AND claim_expires_at > clock_timestamp()
                     AND delivery_id = $8
                     AND prepare_command_id = $9
                     AND prepare_request_digest = $10
                     AND activate_command_id IS NULL"#,
            )
            .bind(activate_command_id.as_uuid())
            .bind(activate_digest.as_slice())
            .bind(action_id.as_uuid())
            .bind(recovery_id.as_uuid())
            .bind(expected_state_version)
            .bind(claim_token)
            .bind(fencing_version)
            .bind(delivery_id)
            .bind(prepare_command_id.as_uuid())
            .bind(prepare_digest.as_slice())
            .execute(&mut **transaction)
            .await
            .map_err(RecoveryActionTransactionError::from)?;
            if result.rows_affected() == 1 {
                return Ok(());
            }

            let current = sqlx::query(
                r#"SELECT recovery_id, execution_state, state_version, claim_token,
                          claim_fencing, delivery_id, prepare_command_id,
                          prepare_request_digest, activate_command_id,
                          activate_request_digest
                   FROM public.settlement_recovery_actions
                   WHERE id = $1
                   FOR UPDATE"#,
            )
            .bind(action_id.as_uuid())
            .fetch_optional(&mut **transaction)
            .await
            .map_err(RecoveryActionTransactionError::from)?
            .ok_or_else(|| repository_error("recovery action 不存在"))?;
            let current_recovery_id: Uuid =
                current.try_get("recovery_id").map_err(map_storage_error)?;
            let current_state: String =
                current.try_get("execution_state").map_err(map_storage_error)?;
            let current_state_version: i64 =
                current.try_get("state_version").map_err(map_storage_error)?;
            let current_claim_token: Option<Uuid> =
                current.try_get("claim_token").map_err(map_storage_error)?;
            let current_fencing: i64 =
                current.try_get("claim_fencing").map_err(map_storage_error)?;
            let current_delivery_id: Option<Uuid> =
                current.try_get("delivery_id").map_err(map_storage_error)?;
            let current_prepare_command: Option<Uuid> =
                current.try_get("prepare_command_id").map_err(map_storage_error)?;
            let current_prepare_digest: Option<Vec<u8>> =
                current.try_get("prepare_request_digest").map_err(map_storage_error)?;
            let current_activate_command: Option<Uuid> =
                current.try_get("activate_command_id").map_err(map_storage_error)?;
            let current_activate_digest: Option<Vec<u8>> =
                current.try_get("activate_request_digest").map_err(map_storage_error)?;
            if current_recovery_id == recovery_id.as_uuid()
                && current_state == "enqueued"
                && current_state_version == state_version
                && current_claim_token == Some(claim_token)
                && current_fencing == fencing_version
                && current_delivery_id == Some(delivery_id)
                && current_prepare_command == Some(prepare_command_id.as_uuid())
                && current_prepare_digest.as_deref() == Some(prepare_digest.as_slice())
                && current_activate_command == Some(activate_command_id.as_uuid())
                && current_activate_digest.as_deref() == Some(activate_digest.as_slice())
            {
                return Ok(());
            }
            if current_delivery_id.is_some_and(|value| value != delivery_id)
                || current_prepare_command
                    .is_some_and(|value| value != prepare_command_id.as_uuid())
                || current_prepare_digest.as_deref().is_some_and(|value| value != prepare_digest)
                || current_activate_command
                    .is_some_and(|value| value != activate_command_id.as_uuid())
                || current_activate_digest.as_deref().is_some_and(|value| value != activate_digest)
            {
                return Err(repository_error("recovery action activation identity 冲突").into());
            }
            Err(repository_error("recovery action activation fence 已失效").into())
        })
    })
    .await?;
    Ok(RecoveryActionClaim {
        execution_state: "enqueued".to_owned(),
        activate_command_id: Some(activate_command_id),
        activate_request_hash: Some(activate_request_hash),
        state_version,
        ..claim.clone()
    })
}
