//! Operations alert delivery 批次的入队、失败和终态写入。
//!
//! delivery 状态和 signal 冷却必须在同一 PostgreSQL 事务中更新，并追加 delivery 审计事实。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use super::*;

use crate::database::delivery_audit::{DeliveryAudit, FAIL, append as append_delivery_audit};

/// 终结可证明从未激活的告警投递；保持零发送尝试和空执行时间，并原子追加失败审计。
pub(super) async fn terminalize_prepared_delivery(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    delivery_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
    failure_code: &str,
) -> Result<(), ApplicationError> {
    if !matches!(failure_code, "dependency_mismatch" | "retry_exhausted") {
        return Err(storage_error("operations alert prepared delivery 错误码无效"));
    }
    let updated = sqlx::query(
        "UPDATE public.deliveries
         SET status = 'failed',
             lease_token = NULL,
             lease_expires_at = NULL,
             claimed_at = NULL,
             sending_at = NULL,
             completed_at = GREATEST(updated_at, $2),
             external_reference = NULL,
             failure_code = $3,
             result_metadata = jsonb_build_object(
                 'outcome_unknown', false,
                 'reason_code', 'operations_alert_batch_terminal'
             ),
             updated_at = GREATEST(updated_at, $2)
         WHERE id = $1 AND origin_kind = 'operations_alert' AND status = 'prepared'
           AND lease_token IS NULL AND lease_expires_at IS NULL
           AND claimed_at IS NULL AND sending_at IS NULL AND completed_at IS NULL
           AND external_reference IS NULL AND failure_code IS NULL AND activation_command_id IS NULL
           AND activation_request_digest IS NULL AND activated_at IS NULL
           AND fencing_version = 0 AND attempt_count = 0",
    )
    .bind(delivery_id)
    .bind(now)
    .bind(failure_code)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "收口 operations alert prepared delivery 失败"))?;
    if updated.rows_affected() == 1 {
        let request_id = delivery_id.to_string();
        append_delivery_audit(
            transaction,
            DeliveryAudit {
                occurred_at: now,
                actor_type: "system",
                actor_id: "delivery-service",
                action: FAIL,
                delivery_id,
                request_id: Some(&request_id),
                source: "system",
                metadata: serde_json::json!({
                    "status": "failed",
                    "failure_code": failure_code,
                    "reason_code": "operations_alert_batch_terminal",
                    "outcome_unknown": false,
                }),
            },
        )
        .await
        .map_err(|error| {
            storage_error_from(error, "追加 operations alert prepared delivery 收口审计失败")
        })?;
    }
    Ok(())
}

pub(super) async fn mark_enqueued_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    claim: &OperationsAlertBatchClaim,
    cooldown: Duration,
) -> Result<(), ApplicationError> {
    let updated_at = Utc::now();
    let next_notify_at = updated_at
        .checked_add_signed(chrono_duration(cooldown)?)
        .ok_or_else(|| storage_error("operations alert 冷却时间溢出"))?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 operations alert 入队确认事务失败"))?;
    lock_batch(&mut transaction, claim.batch_id).await?;
    let updated = sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = 'enqueued', claim_token = NULL, claimed_at = NULL,
             claim_expires_at = NULL, next_attempt_at = NULL, delivery_error_code = '',
             updated_at = $5
         WHERE id = $1 AND delivery_id = $2 AND status = 'claimed'
           AND claim_token = $3 AND claim_fencing = $4
           AND claim_expires_at > $5 AND claim_expires_at > clock_timestamp()",
    )
    .bind(claim.batch_id)
    .bind(claim.delivery_id)
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .bind(updated_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 operations alert 入队状态失败"))?;
    if updated.rows_affected() != 1 {
        let _ = transaction.rollback().await;
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    let delivery_updated = sqlx::query(
        "UPDATE public.deliveries
         SET status = 'pending', updated_at = $2
         WHERE id = $1 AND origin_kind = 'operations_alert' AND status = 'prepared'",
    )
    .bind(claim.delivery_id)
    .bind(updated_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 operations alert delivery 待发送状态失败"))?;
    if delivery_updated.rows_affected() != 1 {
        let _ = transaction.rollback().await;
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    sqlx::query(
        "UPDATE public.operations_alert_signals AS signal
         SET last_notified_at = $2, next_notify_at = $3,
             state_version = signal.state_version + 1
         FROM public.operations_alert_batch_items AS item
         WHERE item.batch_id = $1 AND item.fingerprint = signal.fingerprint
           AND signal.active",
    )
    .bind(claim.batch_id)
    .bind(updated_at)
    .bind(next_notify_at)
    .execute(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 operations alert signal 冷却时间失败"))?;
    let request_id = claim.command_id.to_string();
    append_delivery_audit(
        &mut transaction,
        DeliveryAudit {
            occurred_at: updated_at,
            actor_type: "system",
            actor_id: "delivery-service",
            action: ACTIVATE,
            delivery_id: claim.delivery_id,
            request_id: Some(&request_id),
            source: "system",
            metadata: json!({"status": "pending"}),
        },
    )
    .await
    .map_err(|error| storage_error_from(error, "追加 operations alert delivery 激活审计失败"))?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 operations alert 入队确认事务失败"))
}

#[cfg(feature = "postgres")]
pub(super) async fn mark_claim_failed_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    claim: &OperationsAlertBatchClaim,
    error_code: OperationsAlertDispatchErrorCode,
    retry_delay: Duration,
) -> Result<(), ApplicationError> {
    let updated_at = Utc::now();
    let terminal = error_code == OperationsAlertDispatchErrorCode::DependencyMismatch
        || claim.attempt_count >= MAX_ATTEMPTS;
    let retry_at = if terminal {
        None
    } else {
        Some(
            updated_at
                .checked_add_signed(chrono_duration(retry_delay)?)
                .ok_or_else(|| storage_error("operations alert 重试时间溢出"))?,
        )
    };
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 operations alert 失败退避事务失败"))?;
    lock_batch(&mut transaction, claim.batch_id).await?;
    let updated = if terminal {
        let terminal_code = if error_code == OperationsAlertDispatchErrorCode::DependencyMismatch {
            error_code.as_str()
        } else {
            "retry_exhausted"
        };
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'failed', claim_token = NULL, claimed_at = NULL,
                 claim_expires_at = NULL, next_attempt_at = NULL,
                 delivery_error_code = $3, completed_at = $4, updated_at = $4
             WHERE id = $1 AND delivery_id = $2 AND status = 'claimed'
               AND claim_token = $5 AND claim_fencing = $6
               AND claim_expires_at > $4 AND claim_expires_at > clock_timestamp()",
        )
        .bind(claim.batch_id)
        .bind(claim.delivery_id)
        .bind(terminal_code)
        .bind(updated_at)
        .bind(claim.claim_token)
        .bind(claim.claim_fencing)
        .execute(&mut *transaction)
        .await
    } else {
        let retry_at =
            retry_at.ok_or_else(|| storage_error("operations alert 可重试批次缺少退避时间"))?;
        sqlx::query(
            "UPDATE public.operations_alert_batches
             SET status = 'pending', claim_token = NULL, claimed_at = NULL,
                 claim_expires_at = NULL, next_attempt_at = $3,
                 delivery_error_code = '', completed_at = NULL, updated_at = $4
             WHERE id = $1 AND delivery_id = $2 AND status = 'claimed'
               AND claim_token = $5 AND claim_fencing = $6
               AND claim_expires_at > $4 AND claim_expires_at > clock_timestamp()
               AND attempt_count < 3",
        )
        .bind(claim.batch_id)
        .bind(claim.delivery_id)
        .bind(retry_at)
        .bind(updated_at)
        .bind(claim.claim_token)
        .bind(claim.claim_fencing)
        .execute(&mut *transaction)
        .await
    }
    .map_err(|error| storage_error_from(error, "写入 operations alert 失败退避状态失败"))?;
    if updated.rows_affected() != 1 {
        let _ = transaction.rollback().await;
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::StaleClaim));
    }
    if terminal {
        terminalize_prepared_delivery(
            &mut transaction,
            claim.delivery_id,
            updated_at,
            if error_code == OperationsAlertDispatchErrorCode::DependencyMismatch {
                "dependency_mismatch"
            } else {
                "retry_exhausted"
            },
        )
        .await?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 operations alert 失败退避事务失败"))
}

/// batch → delivery → signal 锁序不变；持锁后检查实时时钟，避免排队跨过 claim 截止。
async fn lock_batch(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    batch_id: uuid::Uuid,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT id FROM public.operations_alert_batches WHERE id = $1 FOR UPDATE")
        .bind(batch_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|error| storage_error_from(error, "锁定 operations alert 批次租约失败"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
pub(super) async fn list_enqueued_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    limit: u16,
) -> Result<Vec<OperationsAlertEnqueuedBatch>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT batch.id AS batch_id, batch.delivery_id, delivery.status
         FROM public.operations_alert_batches AS batch
         INNER JOIN public.deliveries AS delivery ON delivery.id = batch.delivery_id
         WHERE batch.status = 'enqueued'
           AND delivery.status IN ('succeeded', 'failed', 'manual_reconcile')
         ORDER BY delivery.completed_at, batch.id
         LIMIT $1",
    )
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 已入队对账项失败"))?;
    rows.into_iter()
        .map(|row| {
            let status: String = row.try_get("status").map_err(|error| {
                storage_error_from(error, "读取 operations alert delivery 状态失败")
            })?;
            let delivery_status = parse_delivery_status(&status)
                .ok_or_else(|| storage_error("operations alert delivery 终态未知"))?;
            Ok(OperationsAlertEnqueuedBatch {
                batch_id: row.try_get("batch_id").map_err(|error| {
                    storage_error_from(error, "读取 operations alert 批次 ID 失败")
                })?,
                delivery_id: row.try_get("delivery_id").map_err(|error| {
                    storage_error_from(error, "读取 operations alert delivery ID 失败")
                })?,
                delivery_status,
            })
        })
        .collect()
}

#[cfg(feature = "postgres")]
fn parse_delivery_status(value: &str) -> Option<OperationsAlertDeliveryStatus> {
    match value {
        "succeeded" => Some(OperationsAlertDeliveryStatus::Succeeded),
        "failed" => Some(OperationsAlertDeliveryStatus::Failed),
        "manual_reconcile" => Some(OperationsAlertDeliveryStatus::ManualReconcile),
        _ => None,
    }
}

#[cfg(feature = "postgres")]
pub(super) async fn mark_terminal_postgres(
    pool: &super::TrackedPool<sqlx::Postgres>,
    batch: &OperationsAlertEnqueuedBatch,
    status: OperationsAlertDeliveryStatus,
    completed_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    if batch.delivery_status != status {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    let stored_error = match status {
        OperationsAlertDeliveryStatus::Succeeded => "",
        OperationsAlertDeliveryStatus::Failed => "delivery_failed",
        OperationsAlertDeliveryStatus::ManualReconcile => "delivery_manual_reconcile",
    };
    let updated = sqlx::query(
        "UPDATE public.operations_alert_batches
         SET status = $3, completed_at = $4, updated_at = $4,
             delivery_error_code = $5
         WHERE id = $1 AND delivery_id = $2 AND status = 'enqueued'
           AND EXISTS (
               SELECT 1 FROM public.deliveries AS delivery
               WHERE delivery.id = $2 AND delivery.status = $3
           )",
    )
    .bind(batch.batch_id)
    .bind(batch.delivery_id)
    .bind(status.as_str())
    .bind(completed_at)
    .bind(stored_error)
    .execute(pool)
    .await
    .map_err(|error| storage_error_from(error, "写入 operations alert 终态失败"))?;
    if updated.rows_affected() != 1 {
        return Err(ApplicationError::OperationsAlert(OperationsAlertError::IdentityConflict));
    }
    Ok(())
}
