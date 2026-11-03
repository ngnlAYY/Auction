//! PostgreSQL delivery Admin 人工命令事务与结算履约联动。

use auction_application::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminResult,
    DeliveryAdminSnapshot, RequestHash,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::mapping::{
    decode_result_payload, legacy_result_payload, stored_snapshot, valid_snapshot,
};
use super::transaction::{
    DeliveryAdminTransactionError, PostgresDeliveryAdminTransaction, wait_before_retry,
};
use crate::database::TrackedPool;
use crate::database::delivery_audit::{
    ADMIN_RECONCILE, ADMIN_RELEASE, ADMIN_RETRY_FAILED, DeliveryAudit,
    append as append_delivery_audit,
};

pub(crate) async fn execute(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAdminCommand,
) -> Result<DeliveryAdminResult, DeliveryAdminError> {
    pool.observe_transaction(
        auction_observability::DatabaseTransactionOwner::DeliveryAdmin,
        async {
            const MAX_ATTEMPTS: usize = 3;

            for attempt in 0..MAX_ATTEMPTS {
                match execute_transaction(pool, command).await {
                    Ok(result) => return Ok(result),
                    Err(error) if error.is_retryable() && attempt + 1 < MAX_ATTEMPTS => {
                        // 每次重试都从新的事务开始；已执行的写入不能在同一事务中继续使用。
                        wait_before_retry(attempt).await;
                    }
                    Err(error) => {
                        crate::database::observe_retry_exhaustion(
                            attempt + 1,
                            MAX_ATTEMPTS,
                            error.is_retryable(),
                        );
                        return Err(error.into_delivery_error());
                    }
                }
            }
            // 循环的最后一次尝试总会返回；这里只为未来改动留下失败关闭的出口，不以 panic 终止请求。
            Err(DeliveryAdminError::Unavailable)
        },
    )
    .await
}

async fn execute_transaction(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAdminCommand,
) -> Result<DeliveryAdminResult, DeliveryAdminTransactionError> {
    let mut transaction = PostgresDeliveryAdminTransaction::begin(pool).await?;
    lock_command(&mut transaction, command.command_id).await?;
    if let Some(result) = existing_command(&mut transaction, command).await? {
        transaction.commit().await?;
        return Ok(result);
    }

    // 已提交的原命令先重放；新的人工命令不能改写已有资金绑定的出站责任。
    // 本轮五种动作均拒绝，避免delivery→fulfillment/recovery反锁及凭声明释放/重发。
    let funded = crate::database::settlement_funding::lock_funded_delivery_account(
        transaction.transaction_mut()?,
        command.delivery_id,
    )
    .await
    .map_err(DeliveryAdminTransactionError::Funding)?;
    if funded.is_some() {
        return Err(DeliveryAdminError::Invalid.into());
    }

    let retries_delivery = matches!(
        command.action,
        DeliveryAdminAction::RetryFailed | DeliveryAdminAction::ReconcileRetry
    );
    if retries_delivery {
        crate::database::recovery_commands::lock_recovery_delivery_retry_scope(
            transaction.transaction_mut()?,
            command.delivery_id,
        )
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?;
    }

    // watch 重试不能倒取 lot 锁，更不能通过人工 release 恢复已经取消的领先周期。
    crate::database::lot_watch::lock_delivery_lot(
        transaction.transaction_mut()?,
        command.delivery_id,
    )
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if retries_delivery {
        crate::database::recovery_commands::lock_recovery_delivery_retry_actions(
            transaction.transaction_mut()?,
            command.delivery_id,
        )
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?;
    }
    let row = sqlx::query(
        r#"SELECT id, command_id, request_digest, activation_command_id,
                  activation_request_digest, origin_kind, origin_id, recipient, payload,
                  result_metadata, status,
                  lease_token,
                  lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  activated_at, created_at, updated_at
           FROM public.deliveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(command.delivery_id)
    .fetch_optional(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?
    .ok_or(DeliveryAdminError::NotFound)?;
    let current = stored_snapshot(&row).map_err(DeliveryAdminTransactionError::Business)?;
    // 兼容滚动升级期间尚未取得 advisory lock 的旧实例：delivery 行锁等待结束后重新读取
    // 命令账本，必须先识别重放/冲突，再判断已经被前一命令推进的 fencing version。
    if let Some(result) = existing_command(&mut transaction, command).await? {
        transaction.commit().await?;
        return Ok(result);
    }
    // 旧投递已由追加修复事实永久退役；当前 fulfillment 指针复建后也不能把它当作普通孤儿重发。
    // 原成功命令已在上方重放；这里只拒绝新动作，因此不依赖用于放行的 trusted 证明匹配。
    let retired: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_legacy_self_reconciliations WHERE delivery_id=$1)
          OR EXISTS(SELECT 1 FROM public.settlement_equipment_losses WHERE delivery_id=$1)",
    )
    .bind(command.delivery_id)
    .fetch_one(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if retired {
        return Err(DeliveryAdminError::Invalid.into());
    }
    if current.fencing_version != command.expected_fencing_version {
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::Stale));
    }
    // Worker 已在完整 Sent 中目击同一结算义务主题时，重试可能重复寄出现金或装备，判失败会
    // 把已成交误记为未发，两者失败关闭并转人工 Sent 证明；目击只由当前人工核对行的维护写入。
    if matches!(
        command.action,
        DeliveryAdminAction::ReconcileRetry | DeliveryAdminAction::ReconcileFailed
    ) {
        let sighted: bool = sqlx::query_scalar(
            "SELECT COALESCE(result_metadata,'{}'::jsonb) ? 'sent_reference_sighting'
             FROM public.deliveries
             WHERE id=$1 AND origin_kind='settlement' AND payload->>'operation'='send'",
        )
        .bind(command.delivery_id)
        .fetch_optional(transaction.connection()?)
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?
        .unwrap_or(false);
        if sighted {
            return Err(DeliveryAdminError::SentSighted.into());
        }
    }
    if command.action == DeliveryAdminAction::ReconcileSucceeded {
        let payload: serde_json::Value =
            row.try_get("payload").map_err(DeliveryAdminTransactionError::Sqlx)?;
        // 裸 external_reference 不是 Sent 内容证明；人工对账不能绕过主动寄送的成功门。
        if payload.get("operation").and_then(serde_json::Value::as_str) == Some("send") {
            return Err(DeliveryAdminError::Invalid.into());
        }
    }

    if matches!(
        command.action,
        DeliveryAdminAction::Release
            | DeliveryAdminAction::RetryFailed
            | DeliveryAdminAction::ReconcileRetry
    ) && !crate::database::lot_watch::delivery_eligible(
        transaction.transaction_mut()?,
        command.delivery_id,
    )
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?
    {
        return Err(DeliveryAdminError::Stale.into());
    }
    let next_status = apply_delivery_action(&mut transaction, command, &current).await?;
    if command.action == DeliveryAdminAction::RetryFailed
        || command.action == DeliveryAdminAction::ReconcileRetry
    {
        reset_settlement_fulfillment(&mut transaction, command.delivery_id).await?;
        // Recovery owner 在同一命令事务中恢复投影；失败则连 delivery 重试一并回滚。
        if !crate::database::recovery_commands::reset_after_delivery_retry(
            transaction.transaction_mut()?,
            command.delivery_id,
        )
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)?
        {
            return Err(DeliveryAdminError::Stale.into());
        }
    } else if command.action == DeliveryAdminAction::ReconcileSucceeded {
        let external_reference =
            command.external_reference.as_deref().ok_or(DeliveryAdminError::Invalid)?;
        let proof_digest = admin_proof_digest(command.delivery_id, external_reference)?;
        update_settlement_fulfillment(
            &mut transaction,
            command.delivery_id,
            "satisfied",
            None,
            Some(external_reference),
            Some(proof_digest),
        )
        .await?;
    } else if command.action == DeliveryAdminAction::ReconcileFailed {
        update_settlement_fulfillment(
            &mut transaction,
            command.delivery_id,
            "failed",
            command.failure_code.as_deref(),
            None,
            None,
        )
        .await?;
    }

    let updated_row = sqlx::query(
        r#"SELECT id, command_id, request_digest, activation_command_id,
                  activation_request_digest, origin_kind, origin_id, recipient, payload,
                  result_metadata, status,
                  lease_token,
                  lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  activated_at, created_at, updated_at
           FROM public.deliveries
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(command.delivery_id)
    .fetch_one(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    let (payload, snapshot) =
        legacy_result_payload(&updated_row).map_err(DeliveryAdminTransactionError::Business)?;
    if snapshot.status != next_status
        || snapshot.fencing_version <= command.expected_fencing_version
    {
        return Err(DeliveryAdminTransactionError::Business(
            DeliveryAdminError::StoredResultCorrupt,
        ));
    }
    let result = DeliveryAdminResult { command_id: command.command_id, snapshot };
    let inserted = sqlx::query(
        r#"INSERT INTO public.delivery_admin_commands
               (command_id, command_kind, request_digest, delivery_id, result_payload)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (command_id) DO NOTHING"#,
    )
    .bind(command.command_id)
    .bind(command.action.command_kind())
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(command.delivery_id)
    .bind(payload)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if inserted.rows_affected() != 1 {
        // 另一实例可能未使用同一 advisory lock。先回滚本事务中的 delivery/settlement
        // 变更，再从已提交账本恢复原结果；不能把相同命令误报为 conflict。
        transaction.rollback().await?;
        return committed_command(pool, command).await;
    }
    let (audit_action, admin_action) = match command.action {
        DeliveryAdminAction::Release => (ADMIN_RELEASE, "release"),
        DeliveryAdminAction::RetryFailed => (ADMIN_RETRY_FAILED, "retry_failed"),
        DeliveryAdminAction::ReconcileRetry => (ADMIN_RECONCILE, "reconcile_retry"),
        DeliveryAdminAction::ReconcileSucceeded => (ADMIN_RECONCILE, "reconcile_succeeded"),
        DeliveryAdminAction::ReconcileFailed => (ADMIN_RECONCILE, "reconcile_failed"),
    };
    append_delivery_audit(
        transaction.transaction_mut()?,
        DeliveryAudit {
            occurred_at: result.snapshot.updated_at,
            actor_type: "admin",
            actor_id: &command.actor_id,
            action: audit_action,
            delivery_id: command.delivery_id,
            request_id: command.request_id.as_deref(),
            source: "admin",
            metadata: serde_json::json!({
                "command_id": command.command_id,
                "admin_action": admin_action,
                "status": result.snapshot.status,
                "fencing_version": result.snapshot.fencing_version,
            }),
        },
    )
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    transaction.commit().await?;
    Ok(result)
}

/// 以 command ID 为 PostgreSQL 事务锁键，统一同一幂等身份的并发入口。
async fn lock_command(
    transaction: &mut PostgresDeliveryAdminTransaction,
    command_id: Uuid,
) -> Result<(), DeliveryAdminTransactionError> {
    crate::database::lock_command_id(transaction.connection()?, command_id)
        .await
        .map_err(DeliveryAdminTransactionError::Sqlx)
}

/// 唯一冲突后只从新事务读取已提交账本，避免返回当前已回滚事务中的临时结果。
async fn committed_command(
    pool: &TrackedPool<Postgres>,
    command: &DeliveryAdminCommand,
) -> Result<DeliveryAdminResult, DeliveryAdminTransactionError> {
    // 竞态事务已经回滚；账本恢复必须使用另一笔完整 SERIALIZABLE 事务读取已提交结果。
    let mut transaction = PostgresDeliveryAdminTransaction::begin(pool).await?;
    let result = existing_command(&mut transaction, command)
        .await?
        .ok_or(DeliveryAdminError::Unavailable)?;
    transaction.commit().await?;
    Ok(result)
}

async fn existing_command(
    transaction: &mut PostgresDeliveryAdminTransaction,
    command: &DeliveryAdminCommand,
) -> Result<Option<DeliveryAdminResult>, DeliveryAdminTransactionError> {
    let Some(row) = sqlx::query(
        "SELECT command_kind, request_digest, delivery_id, result_payload
         FROM public.delivery_admin_commands WHERE command_id = $1",
    )
    .bind(command.command_id)
    .fetch_optional(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?
    else {
        return Ok(None);
    };
    let kind: String = row.try_get("command_kind").map_err(DeliveryAdminTransactionError::Sqlx)?;
    let digest: Vec<u8> =
        row.try_get("request_digest").map_err(DeliveryAdminTransactionError::Sqlx)?;
    let delivery_id: Uuid =
        row.try_get("delivery_id").map_err(DeliveryAdminTransactionError::Sqlx)?;
    if kind != command.action.command_kind()
        || digest != command.request_hash.as_bytes()
        || delivery_id != command.delivery_id
    {
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::CommandConflict));
    }
    let payload: Value =
        row.try_get("result_payload").map_err(DeliveryAdminTransactionError::Sqlx)?;
    let result = decode_result_payload(payload, command.command_id)?;
    if result.command_id != command.command_id
        || result.snapshot.id != command.delivery_id
        || !valid_snapshot(&result.snapshot)
        || result.snapshot.status != delivery_action_transition(command.action).1
        || result.snapshot.fencing_version <= command.expected_fencing_version
    {
        return Err(DeliveryAdminTransactionError::Business(
            DeliveryAdminError::StoredResultCorrupt,
        ));
    }
    Ok(Some(result))
}

async fn apply_delivery_action(
    transaction: &mut PostgresDeliveryAdminTransaction,
    command: &DeliveryAdminCommand,
    current: &DeliveryAdminSnapshot,
) -> Result<&'static str, DeliveryAdminTransactionError> {
    let (expected_status, next_status) = delivery_action_transition(command.action);
    if current.status != expected_status {
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::Stale));
    }
    let quarantined_mail = current.reason_code.as_deref() == Some("mail_disposition_conflict");
    if quarantined_mail && command.action != DeliveryAdminAction::ReconcileFailed {
        // 隔离发生在发送前；旧 payload 没有重新发送或伪装外部成功的授权。人工核对后
        // 只能终结这一意图，资产修复必须保留独立证据，不能借通用 retry 绕过接受事实。
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::Invalid));
    }
    let (external_reference, failure_code, metadata) = match command.action {
        DeliveryAdminAction::ReconcileSucceeded => (
            command.external_reference.as_deref(),
            None,
            serde_json::json!({"resolution":"admin_reconciled_succeeded"}),
        ),
        DeliveryAdminAction::ReconcileFailed => (
            None,
            command.failure_code.as_deref(),
            if quarantined_mail {
                serde_json::json!({
                    "resolution": "admin_reconciled_failed",
                    "reason_code": "mail_disposition_conflict",
                    "preflight_blocked": true,
                    "outcome_unknown": false,
                })
            } else {
                serde_json::json!({"resolution":"admin_reconciled_failed"})
            },
        ),
        _ => (None, None, serde_json::json!({})),
    };
    let result = sqlx::query(
        r#"UPDATE public.deliveries
           SET status = $2,
               lease_token = NULL,
               lease_expires_at = NULL,
               claimed_at = CASE WHEN $2 = 'pending' THEN NULL ELSE claimed_at END,
               sending_at = CASE WHEN $2 = 'pending' THEN NULL ELSE sending_at END,
               completed_at = CASE
                   WHEN $2 IN ('succeeded', 'failed') THEN clock_timestamp()
                   ELSE NULL
               END,
               external_reference = $3,
               failure_code = $4,
               result_metadata = $5,
               fencing_version = fencing_version + 1,
               updated_at = clock_timestamp()
           WHERE id = $1
             AND status = $6
             AND fencing_version = $7"#,
    )
    .bind(command.delivery_id)
    .bind(next_status)
    .bind(external_reference)
    .bind(failure_code)
    .bind(metadata)
    .bind(expected_status)
    .bind(command.expected_fencing_version)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if result.rows_affected() != 1 {
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::Stale));
    }
    Ok(next_status)
}

/// 返回 Admin 动作唯一允许的起止状态；账本回放必须复用同一映射，不能只验证快照形状。
const fn delivery_action_transition(action: DeliveryAdminAction) -> (&'static str, &'static str) {
    match action {
        DeliveryAdminAction::Release => ("claimed", "pending"),
        DeliveryAdminAction::RetryFailed => ("failed", "pending"),
        DeliveryAdminAction::ReconcileRetry => ("manual_reconcile", "pending"),
        DeliveryAdminAction::ReconcileSucceeded => ("manual_reconcile", "succeeded"),
        DeliveryAdminAction::ReconcileFailed => ("manual_reconcile", "failed"),
    }
}

async fn reset_settlement_fulfillment(
    transaction: &mut PostgresDeliveryAdminTransaction,
    delivery_id: Uuid,
) -> Result<(), DeliveryAdminTransactionError> {
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'enqueued',
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               last_error_code = NULL,
               completed_at = NULL,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE delivery_id = $1
             AND execution_state IN ('failed', 'manual_reconcile')"#,
    )
    .bind(delivery_id)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    ensure_settlement_update_or_non_settlement(transaction, delivery_id, updated.rows_affected())
        .await
}

async fn update_settlement_fulfillment(
    transaction: &mut PostgresDeliveryAdminTransaction,
    delivery_id: Uuid,
    next_state: &str,
    failure_code: Option<&str>,
    proof_reference: Option<&str>,
    proof_digest: Option<RequestHash>,
) -> Result<(), DeliveryAdminTransactionError> {
    let completed_at = Utc::now();
    let proof_digest = proof_digest.map(RequestHash::as_bytes);
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = $2,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               last_error_code = $3,
               proof_kind = CASE WHEN $2 = 'satisfied' THEN 'admin_reconcile' ELSE NULL END,
               proof_reference = $4,
               proof_digest = $5,
               proof_observed_at = CASE WHEN $2 = 'satisfied' THEN $6 ELSE NULL END,
               completed_at = $6,
               state_version = state_version + 1,
               updated_at = CURRENT_TIMESTAMP
           WHERE delivery_id = $1
             AND execution_state = 'manual_reconcile'"#,
    )
    .bind(delivery_id)
    .bind(next_state)
    .bind(failure_code)
    .bind(proof_reference)
    .bind(proof_digest.as_ref().map(|value| value.as_slice()))
    .bind(completed_at)
    .execute(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    ensure_settlement_update_or_non_settlement(transaction, delivery_id, updated.rows_affected())
        .await
}

async fn ensure_settlement_update_or_non_settlement(
    transaction: &mut PostgresDeliveryAdminTransaction,
    delivery_id: Uuid,
    affected: u64,
) -> Result<(), DeliveryAdminTransactionError> {
    let is_settlement: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_obligation_fulfillments WHERE delivery_id = $1)",
    )
    .bind(delivery_id)
    .fetch_one(transaction.connection()?)
    .await
    .map_err(DeliveryAdminTransactionError::Sqlx)?;
    if is_settlement && affected != 1 {
        return Err(DeliveryAdminTransactionError::Business(DeliveryAdminError::Stale));
    }
    Ok(())
}

#[derive(Serialize)]
struct AdminProof<'a> {
    external_reference: &'a str,
}

fn admin_proof_digest(
    delivery_id: Uuid,
    external_reference: &str,
) -> Result<RequestHash, DeliveryAdminError> {
    RequestHash::from_request(
        "admin_reconcile_delivery_proof",
        delivery_id,
        &AdminProof { external_reference },
    )
    .map_err(|_| DeliveryAdminError::Unavailable)
}
