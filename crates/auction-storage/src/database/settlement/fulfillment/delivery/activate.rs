//! 结算履约 prepared delivery 的 activation：确认投递已激活并推进义务状态。

use crate::database::settlement::fulfillment::mapping::{
    repository_error, validate_claim_identity,
};

use crate::database::delivery::audit::{ACTIVATE, DeliveryAudit, append as append_delivery_audit};
use auction_application::settlement::{SettlementFulfillmentClaim, SettlementFulfillmentState};
use auction_domain::CommandId;
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::{DeliveryTransactionError, run_serializable_delivery_transaction};

pub(in crate::database::settlement::fulfillment) async fn activate_postgres(
    pool: &crate::database::TrackedPool<Postgres>,
    claim: &SettlementFulfillmentClaim,
    activate_command_id: CommandId,
    activate_request_hash: auction_application::core::RequestHash,
    activated_at: DateTime<Utc>,
    _next_attempt_at: Option<DateTime<Utc>>,
    custody: Option<(
        &auction_application::settlement::SettlementInternalEvidenceSnapshot,
        std::time::Instant,
    )>,
) -> Result<
    auction_application::settlement::SettlementFulfillmentActivationResult,
    auction_application::core::RepositoryError,
> {
    validate_claim_identity(claim)?;
    let (Some(delivery_id), Some(prepare_command_id), Some(prepare_request_hash)) =
        (claim.delivery_id, claim.prepare_command_id, claim.prepare_request_hash)
    else {
        return Err(repository_error("prepared 履约缺少 delivery prepare identity"));
    };
    if claim.state != SettlementFulfillmentState::Prepared
        || activate_command_id.is_nil()
        || activate_request_hash.as_bytes() == [0; 32]
    {
        return Err(repository_error("结算履约 activation identity 无效"));
    }
    let expected_state_version = i64::try_from(claim.state_version)
        .map_err(|_| repository_error("结算履约状态版本超出数据库范围"))?;
    let next_state_version = claim
        .state_version
        .checked_add(1)
        .ok_or_else(|| repository_error("结算履约状态版本溢出"))?;
    let mut next_delivery_snapshot = claim
        .delivery_snapshot
        .clone()
        .ok_or_else(|| repository_error("prepared 履约缺少 delivery 快照"))?;
    next_delivery_snapshot.status =
        auction_application::settlement::SettlementFulfillmentDeliveryStatus::Pending;
    next_delivery_snapshot.activate_command_id = Some(activate_command_id);
    next_delivery_snapshot.activate_request_hash = Some(activate_request_hash);
    let request_id = activate_command_id.to_string();
    let claim_snapshot = claim.clone();
    let request_id_snapshot = request_id.clone();
    let _activated =
        run_serializable_delivery_transaction(pool, custody, move |transaction, custody| {
            let claim = claim_snapshot.clone();
            let request_id = request_id_snapshot.clone();
            Box::pin(async move {
                super::super::internal::lock_legacy(
                    transaction,
                    &claim,
                    custody.map(|evidence| (evidence, activate_command_id.as_uuid())),
                )
                .await
                .map_err(DeliveryTransactionError::Funding)?;
                let delivery = sqlx::query(
                    r#"SELECT status, activation_command_id, activation_request_digest
                   FROM public.deliveries
                   WHERE id = $1
                   FOR UPDATE"#,
                )
                .bind(delivery_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(DeliveryTransactionError::from)?
                .ok_or_else(|| {
                    DeliveryTransactionError::from(repository_error("prepared delivery 不存在"))
                })?;
                let status: String =
                    delivery.try_get("status").map_err(DeliveryTransactionError::from)?;
                let existing_command: Option<Uuid> = delivery
                    .try_get("activation_command_id")
                    .map_err(DeliveryTransactionError::from)?;
                let existing_digest: Option<Vec<u8>> = delivery
                    .try_get("activation_request_digest")
                    .map_err(DeliveryTransactionError::from)?;
                let activated = match status.as_str() {
                    "prepared" => {
                        if existing_command.is_some() || existing_digest.is_some() {
                            return Err(DeliveryTransactionError::from(repository_error(
                                "delivery activation identity 已被占用",
                            )));
                        }
                        let updated = sqlx::query(
                            r#"UPDATE public.deliveries
                           SET status = 'pending',
                               activation_command_id = $2,
                               activation_request_digest = $3,
                               activated_at = $4,
                               updated_at = CURRENT_TIMESTAMP
                           WHERE id = $1 AND status = 'prepared'"#,
                        )
                        .bind(delivery_id)
                        .bind(activate_command_id.as_uuid())
                        .bind(activate_request_hash.as_bytes().as_slice())
                        .bind(activated_at)
                        .execute(&mut **transaction)
                        .await
                        .map_err(DeliveryTransactionError::from)?;
                        if updated.rows_affected() != 1 {
                            return Err(DeliveryTransactionError::from(repository_error(
                                "delivery 激活状态已变化",
                            )));
                        }
                        true
                    }
                    "pending" | "claimed" | "sending" | "succeeded" | "failed"
                    | "manual_reconcile"
                        if existing_command == Some(activate_command_id.as_uuid())
                            && existing_digest.as_deref()
                                == Some(activate_request_hash.as_bytes().as_slice()) =>
                    {
                        false
                    }
                    _ => {
                        return Err(DeliveryTransactionError::from(repository_error(
                            "delivery 当前状态不允许激活",
                        )));
                    }
                };

                // 激活后继续复用同一 claim 对账；领取期间 next_attempt_at 必须为空。
                // 兼容参数保留，下一共享窗口由对账延期在清 claim 的同一事务内持久化。
                let updated = sqlx::query(
                    r#"UPDATE public.settlement_obligation_fulfillments
                       SET execution_state = 'enqueued',
                           activate_command_id = $2,
                           activate_request_digest = $3,
                           state_version = state_version + 1,
                           next_attempt_at = NULL,
                           last_error_code = NULL,
                           updated_at = CURRENT_TIMESTAMP
                   WHERE obligation_id = $1
                         AND execution_state = 'prepared'
                         AND state_version = $4
                         AND claim_token = $5
                         AND claim_fencing = $6
                         AND claim_expires_at > clock_timestamp()
                         AND delivery_id = $7
                         AND prepare_command_id = $8
                         AND prepare_request_digest = $9"#,
                )
                .bind(claim.obligation_id.as_uuid())
                .bind(activate_command_id.as_uuid())
                .bind(activate_request_hash.as_bytes().as_slice())
                .bind(expected_state_version)
                .bind(claim.claim_token)
                .bind(claim.claim_fencing)
                .bind(delivery_id)
                .bind(prepare_command_id.as_uuid())
                .bind(prepare_request_hash.as_bytes().as_slice())
                .execute(&mut **transaction)
                .await
                .map_err(DeliveryTransactionError::from)?;
                if updated.rows_affected() == 0 {
                    let current = sqlx::query(
                        r#"SELECT execution_state, state_version, claim_token, claim_fencing,
                              delivery_id, activate_command_id, activate_request_digest
                       FROM public.settlement_obligation_fulfillments
                       WHERE obligation_id = $1
                       FOR UPDATE"#,
                    )
                    .bind(claim.obligation_id.as_uuid())
                    .fetch_optional(&mut **transaction)
                    .await
                    .map_err(DeliveryTransactionError::from)?
                    .ok_or_else(|| {
                        DeliveryTransactionError::from(repository_error("结算履约记录不存在"))
                    })?;
                    let current_state: String = current
                        .try_get("execution_state")
                        .map_err(DeliveryTransactionError::from)?;
                    let current_state_version: i64 =
                        current.try_get("state_version").map_err(DeliveryTransactionError::from)?;
                    let current_claim_token: Option<Uuid> =
                        current.try_get("claim_token").map_err(DeliveryTransactionError::from)?;
                    let current_claim_fencing: i64 =
                        current.try_get("claim_fencing").map_err(DeliveryTransactionError::from)?;
                    let current_delivery: Option<Uuid> =
                        current.try_get("delivery_id").map_err(DeliveryTransactionError::from)?;
                    let current_command: Option<Uuid> = current
                        .try_get("activate_command_id")
                        .map_err(DeliveryTransactionError::from)?;
                    let current_digest: Option<Vec<u8>> = current
                        .try_get("activate_request_digest")
                        .map_err(DeliveryTransactionError::from)?;
                    if current_state != "enqueued"
                        || current_state_version
                            != i64::try_from(next_state_version).map_err(|_| {
                                DeliveryTransactionError::from(repository_error(
                                    "结算履约状态版本超出数据库范围",
                                ))
                            })?
                        || current_claim_token != Some(claim.claim_token)
                        || current_claim_fencing != claim.claim_fencing
                        || current_delivery != Some(delivery_id)
                        || current_command != Some(activate_command_id.as_uuid())
                        || current_digest.as_deref()
                            != Some(activate_request_hash.as_bytes().as_slice())
                    {
                        return Err(DeliveryTransactionError::from(repository_error(
                            "结算履约 activation claim 已失效或冲突",
                        )));
                    }
                }
                if activated {
                    append_delivery_audit(
                        &mut *transaction,
                        DeliveryAudit {
                            occurred_at: activated_at,
                            actor_type: "system",
                            actor_id: "delivery-service",
                            action: ACTIVATE,
                            delivery_id,
                            request_id: Some(&request_id),
                            source: "system",
                            metadata: serde_json::json!({"status": "pending"}),
                        },
                    )
                    .await
                    .map_err(DeliveryTransactionError::from)?;
                }
                Ok(activated)
            })
        })
        .await?;
    let mut next_claim = claim.clone();
    next_claim.state = SettlementFulfillmentState::Enqueued;
    next_claim.state_version = next_state_version;
    next_claim.activate_command_id = Some(activate_command_id);
    next_claim.activate_request_hash = Some(activate_request_hash);
    next_claim.delivery_snapshot = Some(next_delivery_snapshot);
    Ok(auction_application::settlement::SettlementFulfillmentActivationResult {
        obligation_id: claim.obligation_id,
        delivery_id,
        activate_command_id,
        next_claim,
    })
}
