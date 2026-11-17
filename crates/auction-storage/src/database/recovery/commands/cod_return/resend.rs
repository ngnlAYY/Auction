//! 未达冻结上限的 COD 退回：恢复 owner 重寄原 COD 投递。

use auction_application::recovery::RecoveryCommandError;
use auction_time::Timestamp;
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::delivery::audit::{
    COD_RETURN_RESEND, DeliveryAudit, append as append_delivery_audit,
};
use crate::database::recovery::commands::mapping::map_sql_error;

/// 未达冻结上限的退回：重置原 COD 投递与履约；追加消费事实保留，原退件不会再次计数。原投递若在
/// 退件到达前被重新排队、因恢复接管而停在待发（当前 fence 从未进入发送围栏），本次重寄以新 fence
/// 接替它，审计 `superseded_queued_intent=true` 记录被接替的待发意图。
pub(super) async fn resend_cod(
    transaction: &mut Transaction<'_, Postgres>,
    now: Timestamp,
    recovery_id: Uuid,
    payment_obligation_id: Uuid,
    state_version: i64,
    payment_deadline_at: Timestamp,
) -> Result<(String, Option<String>, i64), RecoveryCommandError> {
    // 清空当前结果之前冻结安全投影；退件证明只授权本次回收重排，不补造旧 Sent 证明。
    let previous = sqlx::query(
        r#"SELECT delivery.id, delivery.command_id, delivery.fencing_version,
                  jsonb_build_object(
                      'status', 'pending',
                      'original_delivery_id', delivery.id,
                      'original_command_id', delivery.command_id,
                      'previous_status', delivery.status,
                      'superseded_queued_intent', delivery.status IN ('pending', 'claimed'),
                      'previous_fencing_version', delivery.fencing_version,
                      'fencing_version', delivery.fencing_version + 1,
                      'previous_attempt_count', delivery.attempt_count,
                      'previous_claimed_at', delivery.claimed_at,
                      'previous_sending_at', delivery.sending_at,
                      'previous_completed_at', delivery.completed_at,
                      'recovery_id', recovery.id,
                      'payment_obligation_id', recovery.payment_obligation_id,
                      'return_count', recovery.cod_return_count,
                      'return_limit', recovery.cod_return_limit,
                      'return_mail_id', recovery.return_mail_id,
                      'return_external_mail_id', recovery.return_external_mail_id,
                      'return_consumed_at', consumed.recorded_at,
                      'reclaim_action_id', reclaim.id,
                      'reclaim_delivery_id', reclaim.delivery_id,
                      'custody_proof_kind', recovery.custody_proof_kind,
                      'custody_proof_reference', recovery.custody_proof_reference,
                      'custody_proof_digest', encode(recovery.custody_proof_digest, 'hex'),
                      'custody_proof_observed_at', recovery.custody_proof_observed_at,
                      'proven_sent', CASE
                          WHEN delivery.status = 'succeeded'
                           AND delivery.failure_code IS NULL
                           AND delivery.sending_at IS NOT NULL
                           AND delivery.completed_at >= delivery.sending_at
                           AND delivery.result_metadata->>'operation' = 'send'
                           AND delivery.result_metadata->'external_reference_proven' = 'true'::jsonb
                           AND delivery.external_reference ~ '^mooglemail:main:send:[0-9]{1,32}$'
                           AND (delivery.result_metadata->>'proof' = 'sent_recovered_reference_v1'
                                OR (delivery.result_metadata->>'proof' = 'sent_exact_payload_v1'
                                    AND delivery.result_metadata->'sent_verified' = 'true'::jsonb)
                                OR public.delivery_has_frozen_sent_proof(
                                    delivery.id, delivery.result_metadata, delivery.external_reference))
                          THEN jsonb_build_object(
                              'external_reference', delivery.external_reference,
                              'proof', delivery.result_metadata->>'proof')
                          ELSE NULL
                      END
                  ) AS audit_metadata
           FROM public.settlement_obligation_fulfillments AS fulfillment
           JOIN public.deliveries AS delivery ON delivery.id = fulfillment.delivery_id
           JOIN public.settlement_recoveries AS recovery
               ON recovery.id = $2 AND recovery.payment_obligation_id = fulfillment.obligation_id
           JOIN public.settlement_recovery_return_consumptions AS consumed
               ON consumed.recovery_id = recovery.id
              AND consumed.return_mail_id = recovery.return_mail_id
           JOIN public.inbound_mail AS inbound
               ON inbound.id = recovery.return_mail_id
              AND inbound.external_mail_id = recovery.return_external_mail_id
              AND inbound.world_id = 'main'
           JOIN public.settlement_recovery_actions AS reclaim
               ON reclaim.recovery_id = recovery.id
              AND reclaim.kind = 'reclaim_equipment_take'
              AND reclaim.execution_state = 'satisfied'
              AND reclaim.source_mail_id = recovery.return_mail_id
              AND reclaim.proof_kind = 'delivery_recovery_take_succeeded'
              AND reclaim.proof_kind = recovery.custody_proof_kind
              AND reclaim.proof_reference = recovery.return_external_mail_id
              AND reclaim.proof_reference = recovery.custody_proof_reference
              AND reclaim.proof_digest = recovery.custody_proof_digest
           WHERE fulfillment.obligation_id = $1
             AND delivery.origin_kind = 'settlement'
             AND delivery.origin_id = fulfillment.obligation_id::text
             AND (delivery.status IN ('succeeded', 'manual_reconcile')
                  OR (delivery.status IN ('pending', 'claimed') AND delivery.sending_at IS NULL))
             AND recovery.stage = 'reclaiming' AND recovery.state_version = $3
             AND recovery.cod_return_count BETWEEN 1 AND recovery.cod_return_limit - 1
             AND recovery.return_external_mail_id ~ '^[0-9]{1,32}$'
           FOR UPDATE OF delivery"#,
    )
    .bind(payment_obligation_id)
    .bind(recovery_id)
    .bind(state_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    let delivery_id: Uuid = previous.try_get("id").map_err(map_sql_error)?;
    let command_id: Uuid = previous.try_get("command_id").map_err(map_sql_error)?;
    let previous_fencing: i64 = previous.try_get("fencing_version").map_err(map_sql_error)?;
    let audit_metadata: Value = previous.try_get("audit_metadata").map_err(map_sql_error)?;
    let delivery = sqlx::query_scalar::<_, Uuid>(
        r#"UPDATE public.deliveries AS delivery
           SET status = 'pending',
               external_reference = NULL,
               failure_code = NULL,
               completed_at = NULL,
               claimed_at = NULL,
               sending_at = NULL,
               lease_token = NULL,
               lease_expires_at = NULL,
               attempt_count = 0,
               fencing_version = delivery.fencing_version + 1,
               result_metadata = '{}'::jsonb,
               updated_at = $2
           WHERE delivery.id = $1
             AND delivery.fencing_version = $3
             AND (delivery.status IN ('succeeded', 'manual_reconcile')
                  OR (delivery.status IN ('pending', 'claimed') AND delivery.sending_at IS NULL))
           RETURNING delivery.id"#,
    )
    .bind(delivery_id)
    .bind(now)
    .bind(previous_fencing)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    if delivery != delivery_id {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    let request_id = command_id.to_string();
    append_delivery_audit(
        transaction,
        DeliveryAudit {
            occurred_at: now,
            actor_type: "system",
            actor_id: "recovery-service",
            action: COD_RETURN_RESEND,
            delivery_id,
            request_id: Some(&request_id),
            source: "system",
            metadata: audit_metadata,
        },
    )
    .await
    .map_err(map_sql_error)?;

    let fulfillment = sqlx::query_scalar::<_, Uuid>(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET execution_state = 'enqueued',
               state_version = state_version + 1,
               claim_token = NULL,
               claimed_at = NULL,
               claim_expires_at = NULL,
               retry_count = 0,
               next_attempt_at = $2,
               last_error_code = NULL,
               proof_kind = NULL,
               proof_reference = NULL,
               proof_digest = NULL,
               proof_observed_at = NULL,
               completed_at = NULL,
               updated_at = $2
           WHERE obligation_id = $1
             AND execution_state NOT IN ('satisfied', 'failed')
           RETURNING obligation_id"#,
    )
    .bind(payment_obligation_id)
    .bind(now)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::InvalidStoredResult)?;
    let _ = fulfillment;

    // 付款窗口由调用方按 recovery 冻结的策略版本从本次时刻重新起算；已延长的绝对截止不再参与
    // 计算。stage/state_version 围栏保证截止只写入本次锁定的版本。
    let next_state_version: i64 = sqlx::query_scalar(
        r#"UPDATE public.settlement_recoveries
           SET stage = 'awaiting_payment',
               payment_deadline_at = $4,
               reminder_count = 0,
               return_mail_id = NULL,
               return_external_mail_id = NULL,
               custody_proof_kind = NULL,
               custody_proof_reference = NULL,
               custody_proof_digest = NULL,
               custody_proof_observed_at = NULL,
               blocking_code = NULL,
               next_attempt_at = NULL,
               state_version = state_version + 1,
               updated_at = $2
           WHERE id = $1
             AND stage = 'reclaiming'
             AND state_version = $3
           RETURNING state_version"#,
    )
    .bind(recovery_id)
    .bind(now)
    .bind(state_version)
    .bind(payment_deadline_at)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?
    .ok_or(RecoveryCommandError::StaleState)?;

    Ok(("awaiting_payment".to_owned(), None, next_state_version))
}
