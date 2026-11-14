//! 恢复后外部核对的隔离与未决阻塞写入；每条投递独立加锁并追加审计。

use auction_application::core::ApplicationError;
use auction_application::recovery::{
    EXTERNAL_RECOVERY_AMBIGUOUS_REASON, ExternalRecoveryQuarantineMatch,
    ExternalRecoveryQuarantineOutcome, ExternalRecoverySendEvidence,
    ExternalRecoveryUnresolvedBlock,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::{
    EXTERNAL_RECOVERY_BLOCK_ACTION, EXTERNAL_RECOVERY_PAYLOAD_QUARANTINE_FAILURE_CODE,
    EXTERNAL_RECOVERY_QUARANTINE_ACTION, EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE,
    EXTERNAL_RECOVERY_RECEIPT_FORMAT, EXTERNAL_RECOVERY_UNRESOLVED_FAILURE_CODE, load_expectation,
};
use crate::database::delivery::audit::{self as delivery_audit, DeliveryAudit};
use crate::database::{storage_error, storage_error_from};

pub(super) async fn quarantine_one(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    scan_id: Uuid,
    actor_id: &str,
    candidate: &ExternalRecoveryQuarantineMatch,
    now: DateTime<Utc>,
) -> Result<ExternalRecoveryQuarantineOutcome, ApplicationError> {
    let row = sqlx::query(
        "SELECT status, fencing_version FROM public.deliveries WHERE id = $1 FOR UPDATE",
    )
    .bind(candidate.delivery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "锁定恢复隔离投递失败"))?;
    let Some(row) = row else {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "delivery_missing",
        ));
    };
    let status: String =
        row.try_get("status").map_err(|error| storage_error_from(error, "读取投递状态失败"))?;
    let fencing_version: i64 = row
        .try_get("fencing_version")
        .map_err(|error| storage_error_from(error, "读取投递 fence 失败"))?;
    if !matches!(status.as_str(), "pending" | "claimed") {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "not_pending_or_claimed",
        ));
    }
    if fencing_version != candidate.fencing_version {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "fencing_changed",
        ));
    }
    // 恢复库可能没有这次 `sending` 的冻结尝试：备份点仍是 pending/claimed，旧 Worker 在快照
    // 之后才越过发送围栏。此时只能用恢复库 payload 重建的身份复核，结论不能升级为精确证明。
    let Some(expectation) = load_expectation(transaction, candidate.delivery_id).await? else {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "send_identity_unavailable",
        ));
    };
    if expectation.fencing_version != candidate.fencing_version {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "fencing_changed",
        ));
    }
    // 未知装备身份既不能证明一致，也不能当作不一致；其余已知字段必须完全相等。
    let equipment_matches =
        match (expectation.equipment_ids.as_ref(), candidate.expected_equipment_ids.as_ref()) {
            (Some(expected), Some(candidate_ids)) => expected == candidate_ids,
            _ => true,
        };
    if expectation.world_id != candidate.world_id
        || !expectation.recipient.eq_ignore_ascii_case(&candidate.recipient)
        || expectation.subject.as_deref().is_some_and(|subject| subject != candidate.subject)
        || expectation.credits != candidate.expected_credits
        || expectation.cod != candidate.expected_cod
        || !equipment_matches
    {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "send_expectation_changed",
        ));
    }
    // 只有冻结身份才能声称 Sent 精确匹配；payload 重建身份只给出弱匹配结论。
    let (failure_code, evidence_label) = match expectation.evidence {
        ExternalRecoverySendEvidence::Frozen => {
            (EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE, "frozen_expectation")
        }
        ExternalRecoverySendEvidence::RestoredPayload => {
            (EXTERNAL_RECOVERY_PAYLOAD_QUARANTINE_FAILURE_CODE, "restored_payload")
        }
    };

    let inserted = sqlx::query(
        "INSERT INTO public.delivery_external_quarantines
             (scan_id, delivery_id, fencing_version, world_id, external_mail_id, sent_at,
              recipient, subject, collected, observed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (delivery_id, fencing_version) DO NOTHING
         RETURNING id",
    )
    .bind(scan_id)
    .bind(candidate.delivery_id)
    .bind(candidate.fencing_version)
    .bind(&candidate.world_id)
    .bind(&candidate.external_mail_id)
    .bind(candidate.sent_at)
    .bind(&candidate.recipient)
    .bind(&candidate.subject)
    .bind(candidate.collected)
    .bind(candidate.observed_at)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入投递外部隔离事实失败"))?;
    if inserted.is_none() {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "already_quarantined",
        ));
    }

    let updated = sqlx::query(
        "UPDATE public.deliveries AS delivery
         SET status = 'manual_reconcile',
             lease_token = NULL,
             lease_expires_at = NULL,
             completed_at = GREATEST($3::timestamptz, delivery.created_at,
                                     COALESCE(delivery.claimed_at, delivery.created_at)),
             failure_code = $4,
             result_metadata = (
                 CASE WHEN delivery.result_metadata ? 'sweep_retries'
                      THEN jsonb_build_object('sweep_retries',
                                              delivery.result_metadata->'sweep_retries')
                      ELSE '{}'::jsonb END)
                 || jsonb_build_object(
                        'outcome_unknown', true,
                        'reason_code', $4,
                        'external_recovery_quarantine',
                        jsonb_build_object(
                            'format', $5,
                            'scan_id', $1::uuid,
                            'external_mail_id', $6,
                            'sent_at', $7,
                            'observed_at', $8,
                            'match_evidence', $10)),
             fencing_version = delivery.fencing_version + 1,
             updated_at = clock_timestamp()
         WHERE delivery.id = $2
           AND delivery.status IN ('pending', 'claimed')
           AND delivery.fencing_version = $9
         RETURNING delivery.fencing_version, delivery.attempt_count",
    )
    .bind(scan_id)
    .bind(candidate.delivery_id)
    .bind(now)
    .bind(failure_code)
    .bind(EXTERNAL_RECOVERY_RECEIPT_FORMAT)
    .bind(&candidate.external_mail_id)
    .bind(candidate.sent_at)
    .bind(candidate.observed_at)
    .bind(candidate.fencing_version)
    .bind(evidence_label)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "隔离恢复投递失败"))?;
    let Some(updated) = updated else {
        // 已插入隔离事实但状态未推进；整体回滚，避免留下无状态变化的孤证。
        return Err(storage_error("隔离投递时状态在事务内发生变化"));
    };
    let next_fencing: i64 = updated
        .try_get("fencing_version")
        .map_err(|error| storage_error_from(error, "读取隔离后 fence 失败"))?;
    let attempt_count: i32 = updated
        .try_get("attempt_count")
        .map_err(|error| storage_error_from(error, "读取隔离后尝试次数失败"))?;

    delivery_audit::append(
        transaction,
        DeliveryAudit {
            occurred_at: now,
            // audit_records 的 actor_type 闭集不含 cli；CLI 运维动作按 admin 记录，source 仍为 cli。
            actor_type: "admin",
            actor_id,
            action: EXTERNAL_RECOVERY_QUARANTINE_ACTION,
            delivery_id: candidate.delivery_id,
            request_id: Some(&scan_id.to_string()),
            source: "cli",
            metadata: serde_json::json!({
                "status": "manual_reconcile",
                "failure_code": failure_code,
                "fencing_version": next_fencing,
                "previous_fencing_version": candidate.fencing_version,
                "attempt_count": attempt_count,
                "external_mail_id": candidate.external_mail_id,
                "sent_at": candidate.sent_at,
                "observed_at": candidate.observed_at,
                "format": EXTERNAL_RECOVERY_RECEIPT_FORMAT,
                "match_evidence": evidence_label,
            }),
        },
    )
    .await
    .map_err(|error| storage_error_from(error, "写入恢复隔离审计失败"))?;
    Ok(ExternalRecoveryQuarantineOutcome::quarantined(candidate.delivery_id, next_fencing))
}

/// 没有唯一外部证据的未决投递：只做本地阻塞，不写 `delivery_external_quarantines`。
/// 同一 MID 只能证明一条投递，因此多条候选共用证据时必须只保留“未决”结论。
pub(super) async fn block_unresolved_one(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    scan_id: Uuid,
    actor_id: &str,
    block: &ExternalRecoveryUnresolvedBlock,
    now: DateTime<Utc>,
) -> Result<ExternalRecoveryQuarantineOutcome, ApplicationError> {
    let row = sqlx::query(
        "SELECT status, fencing_version FROM public.deliveries WHERE id = $1 FOR UPDATE",
    )
    .bind(block.delivery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "锁定恢复阻塞投递失败"))?;
    let Some(row) = row else {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            block.delivery_id,
            "delivery_missing",
        ));
    };
    let status: String =
        row.try_get("status").map_err(|error| storage_error_from(error, "读取投递状态失败"))?;
    let fencing_version: i64 = row
        .try_get("fencing_version")
        .map_err(|error| storage_error_from(error, "读取投递 fence 失败"))?;
    if !matches!(status.as_str(), "pending" | "claimed") {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            block.delivery_id,
            "not_pending_or_claimed",
        ));
    }
    if fencing_version != block.fencing_version {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            block.delivery_id,
            "fencing_changed",
        ));
    }
    debug_assert_eq!(block.reason, EXTERNAL_RECOVERY_AMBIGUOUS_REASON);
    let updated = sqlx::query(
        "UPDATE public.deliveries AS delivery
         SET status = 'manual_reconcile',
             lease_token = NULL,
             lease_expires_at = NULL,
             completed_at = GREATEST($3::timestamptz, delivery.created_at,
                                     COALESCE(delivery.claimed_at, delivery.created_at)),
             failure_code = $4,
             result_metadata = (
                 CASE WHEN delivery.result_metadata ? 'sweep_retries'
                      THEN jsonb_build_object('sweep_retries',
                                              delivery.result_metadata->'sweep_retries')
                      ELSE '{}'::jsonb END)
                 || jsonb_build_object(
                        'outcome_unknown', true,
                        'reason_code', $4,
                        'external_recovery_unresolved',
                        jsonb_build_object(
                            'format', $5,
                            'scan_id', $1::uuid,
                            'reason', $6,
                            'candidate_mail_ids', $7)),
             fencing_version = delivery.fencing_version + 1,
             updated_at = clock_timestamp()
         WHERE delivery.id = $2
           AND delivery.status IN ('pending', 'claimed')
           AND delivery.fencing_version = $8
         RETURNING delivery.fencing_version, delivery.attempt_count",
    )
    .bind(scan_id)
    .bind(block.delivery_id)
    .bind(now)
    .bind(EXTERNAL_RECOVERY_UNRESOLVED_FAILURE_CODE)
    .bind(EXTERNAL_RECOVERY_RECEIPT_FORMAT)
    .bind(&block.reason)
    .bind(serde_json::json!(block.candidate_mail_ids))
    .bind(block.fencing_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "阻塞未决恢复投递失败"))?;
    let Some(updated) = updated else {
        return Err(storage_error("阻塞未决投递时状态在事务内发生变化"));
    };
    let next_fencing: i64 = updated
        .try_get("fencing_version")
        .map_err(|error| storage_error_from(error, "读取阻塞后 fence 失败"))?;
    let attempt_count: i32 = updated
        .try_get("attempt_count")
        .map_err(|error| storage_error_from(error, "读取阻塞后尝试次数失败"))?;
    delivery_audit::append(
        transaction,
        DeliveryAudit {
            occurred_at: now,
            // audit_records 的 actor_type 闭集不含 cli；CLI 运维动作按 admin 记录，source 仍为 cli。
            actor_type: "admin",
            actor_id,
            action: EXTERNAL_RECOVERY_BLOCK_ACTION,
            delivery_id: block.delivery_id,
            request_id: Some(&scan_id.to_string()),
            source: "cli",
            metadata: serde_json::json!({
                "status": "manual_reconcile",
                "failure_code": EXTERNAL_RECOVERY_UNRESOLVED_FAILURE_CODE,
                "fencing_version": next_fencing,
                "previous_fencing_version": block.fencing_version,
                "attempt_count": attempt_count,
                "reason": block.reason,
                "candidate_mail_ids": block.candidate_mail_ids,
                "format": EXTERNAL_RECOVERY_RECEIPT_FORMAT,
            }),
        },
    )
    .await
    .map_err(|error| storage_error_from(error, "写入恢复阻塞审计失败"))?;
    Ok(ExternalRecoveryQuarantineOutcome::blocked(block.delivery_id, next_fencing))
}
