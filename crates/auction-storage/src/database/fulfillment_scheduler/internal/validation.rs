//! 在全部业务锁下重核当前用户、原义务、租约、依赖和完整 opaque 证明。

use super::sql_error;
use auction_application::{
    FundingError, InternalEvidencePurpose, SettlementInternalCommand,
    SettlementInternalEvidenceSnapshot,
};
use auction_domain::{InternalSettlementAction, SettlementObligationKind};
use sqlx::{Postgres, Row, Transaction};

pub(super) async fn validate(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    collected: bool,
    external_refund: bool,
) -> Result<(), FundingError> {
    let c = &command.claim;
    let rejected = |kind, error| {
        if collected {
            crate::database::settlement_funding::diagnostics::reject(kind, "claim_validate", error)
        } else {
            error
        }
    };
    let rejection = if !InternalSettlementAction::matches_frozen_shape(
        c.kind,
        c.world,
        c.equipment_world,
        c.amount,
        c.carrier_credit,
    ) {
        Some("collected_claim_shape_invalid")
    } else if !evidence.validate() {
        Some("collected_claim_snapshot_invalid")
    } else if evidence.observed_from < c.claimed_at {
        Some("collected_claim_observation_before_claim")
    } else if evidence.operation_id != command.command_id {
        Some("collected_claim_operation_mismatch")
    } else if evidence.identity.world != c.world {
        Some("collected_claim_world_mismatch")
    } else if !matches!(&evidence.purpose, InternalEvidencePurpose::SettlementClaim {
            obligation_id, claim_token, claim_fencing, state_version
        } if *obligation_id==c.obligation_id.as_uuid() && *claim_token==c.claim_token
            && *claim_fencing==c.claim_fencing && *state_version==c.state_version)
    {
        Some("collected_claim_binding_mismatch")
    } else if !collected && !external_refund && evidence.identity.player_name != c.recipient {
        Some("collected_claim_recipient_mismatch")
    } else if external_refund && c.kind != SettlementObligationKind::BuyerRefund {
        Some("collected_claim_refund_kind_invalid")
    } else {
        None
    };
    if let Some(kind) = rejection {
        return Err(rejected(kind, FundingError::Unproven));
    }
    let prepare_digest = c.prepare_request_hash.map(|hash| hash.as_bytes());
    let activate_digest = c.activate_request_hash.map(|hash| hash.as_bytes());
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_obligations o
         JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
         JOIN public.equipment e ON e.id=o.equipment_id
         JOIN public.lots l ON l.id=o.lot_id AND l.auction_id=o.auction_id
         WHERE o.id=$1 AND o.auction_id=$2 AND o.lot_id=$3 AND o.origin_command_id=$4
           AND o.obligation_kind=$5 AND o.amount=$6 AND o.carrier_credit=$7
           AND o.equipment_id=$8 AND e.external_equipment_id=$9 AND e.world_id=$10
           AND o.recipient_user_id IS NOT DISTINCT FROM $11
           AND o.prerequisite_obligation_id IS NOT DISTINCT FROM $12
           AND o.business_state='ready' AND f.execution_state=$13
           AND f.state_version=$14 AND f.claim_token=$15 AND f.claim_fencing=$16
           AND f.claim_expires_at>clock_timestamp() AND f.claim_expires_at=$23 AND f.dispatch_world=$17
           AND f.delivery_operation='send'
           AND f.delivery_id IS NOT DISTINCT FROM $18
           AND (o.recipient_user_id IS NOT NULL OR o.recipient_game_identity=$19)
           AND f.prepare_command_id IS NOT DISTINCT FROM $20
           AND f.activate_command_id IS NOT DISTINCT FROM $21
           AND o.submission_id IS NOT DISTINCT FROM $22
           AND f.prepare_request_digest IS NOT DISTINCT FROM $24
           AND f.activate_request_digest IS NOT DISTINCT FROM $25
           AND f.proof_kind IS NULL)"
    ).bind(c.obligation_id.as_uuid()).bind(c.auction_id.as_uuid()).bind(c.lot_id.as_uuid())
        .bind(c.origin_command_id.as_uuid()).bind(c.kind.as_str()).bind(c.amount).bind(i64::from(c.carrier_credit))
        .bind(c.equipment_id.as_uuid()).bind(&c.external_equipment_id).bind(c.equipment_world.as_str())
        .bind(c.recipient_user_id).bind(c.prerequisite_id.map(|id| id.as_uuid()))
        .bind(if collected { "enqueued" } else { "pending" })
        .bind(i64::try_from(c.state_version).map_err(|_| rejected("collected_claim_version_invalid", FundingError::Invalid))?)
        .bind(c.claim_token).bind(c.claim_fencing).bind(c.world.as_str()).bind(c.delivery_id)
        .bind(&c.recipient).bind(c.prepare_command_id.map(|id| id.as_uuid()))
        .bind(c.activate_command_id.map(|id| id.as_uuid())).bind(c.submission_id.map(|id| id.as_uuid()))
        .bind(c.lease_until)
        .bind(prepare_digest.as_ref().map(|value| value.as_slice()))
        .bind(activate_digest.as_ref().map(|value| value.as_slice()))
        .fetch_one(&mut **tx).await.map_err(sql_error)
        .map_err(|error| rejected("collected_claim_query_failed", error))?;
    if !valid {
        return Err(rejected("collected_claim_conflict", FundingError::Conflict));
    }
    if let Some(user_id) = c.recipient_user_id {
        let user =
            sqlx::query("SELECT username,status,state_version FROM public.users WHERE id=$1")
                .bind(user_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(sql_error)
                .map_err(|error| rejected("collected_claim_user_query_failed", error))?;
        // 实收只验证 UUID 仍存在；地址由已发送 delivery 与 Sent 证明精确绑定。
        if !collected
            && (user.try_get::<String, _>("username").map_err(sql_error)? != c.recipient
                || user.try_get::<String, _>("status").map_err(sql_error)? != "active"
                || command.recipient_version
                    != u64::try_from(user.try_get::<i64, _>("state_version").map_err(sql_error)?)
                        .ok())
        {
            return Err(FundingError::Conflict);
        }
    }
    if let Some(prerequisite) = c.prerequisite_id {
        let satisfied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.settlement_obligation_fulfillments WHERE obligation_id=$1 AND execution_state='satisfied' AND proof_digest IS NOT NULL)")
            .bind(prerequisite.as_uuid()).fetch_one(&mut **tx).await.map_err(sql_error)
            .map_err(|error| rejected("collected_prerequisite_query_failed", error))?;
        if !satisfied {
            return Err(rejected("collected_prerequisite_unproven", FundingError::Unproven));
        }
    }
    if !collected {
        let action = InternalSettlementAction::for_kind(c.kind).ok_or(FundingError::Unsupported)?;
        if c.delivery_id.is_some()
            || c.prepare_command_id.is_some()
            || c.activate_command_id.is_some()
            || c.prepare_request_hash.is_some()
            || c.activate_request_hash.is_some()
        {
            return Err(FundingError::Conflict);
        }
        if action == InternalSettlementAction::KeepUnsold {
            let source: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.auction_submissions s WHERE s.id=$1 AND s.source_kind IN ('mail','admin') AND s.source_kind=$2 AND s.seller=$3 AND s.external_mail_id IS NOT DISTINCT FROM $4)")
                .bind(c.submission_id.map(|id| id.as_uuid())).bind(&c.submission_source_kind)
                .bind(&c.recipient).bind(&c.submission_external_mail_id)
                .fetch_one(&mut **tx).await.map_err(sql_error)?;
            if !source {
                return Err(FundingError::Unproven);
            }
        }
        if action.requires_equipment() {
            let e = evidence.equipment.as_ref().ok_or(FundingError::Unproven)?;
            if e.equipment_id != c.equipment_id
                || e.external_equipment_id != c.external_equipment_id
                || e.binding.world != c.equipment_world
            {
                return Err(FundingError::Unproven);
            }
        }
        if action == InternalSettlementAction::Handoff {
            // 原付款必须有本lane完整同账号证明；不凭旧无UID的collected字段推断当前账号实收。
            let paid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.settlement_internal_commands WHERE obligation_id=$1 AND action IN ('internal','collected') AND uid=$2)")
                .bind(c.prerequisite_id.map(|id| id.as_uuid())).bind(&evidence.identity.authenticated_uid)
                .fetch_one(&mut **tx).await.map_err(sql_error)?;
            if !paid {
                return Err(FundingError::Unproven);
            }
        }
        if action == InternalSettlementAction::Refund {
            let recovered: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM public.settlement_recoveries r
                 JOIN public.settlement_obligation_fulfillments p ON p.obligation_id=r.payment_obligation_id
                 JOIN public.settlement_obligation_fulfillments h ON h.obligation_id=r.equipment_obligation_id
                 JOIN public.deliveries hd ON hd.id=h.delivery_id
                 JOIN public.settlement_recovery_actions a ON a.recovery_id=r.id AND a.kind='recover_equipment_take'
                 WHERE r.buyer_refund_obligation_id=$1 AND r.stage='refunding' AND r.resolution='equipment_recovered'
                   AND r.custody_proof_digest IS NOT NULL AND r.blocking_code IS NULL
                   AND a.execution_state='satisfied' AND a.proof_digest=r.custody_proof_digest
                   AND h.execution_state='enqueued' AND hd.status='succeeded'
                   AND (hd.result_metadata->>'proof' IN ('sent_exact_payload_v1','sent_recovered_reference_v1')
                     OR public.delivery_has_frozen_sent_proof(hd.id,hd.result_metadata,hd.external_reference))
                   AND hd.result_metadata->>'external_reference_proven'='true'
                   AND p.execution_state='satisfied' AND p.proof_digest IS NOT NULL)"
            ).bind(c.obligation_id.as_uuid()).fetch_one(&mut **tx).await.map_err(sql_error)?;
            if !recovered {
                return Err(FundingError::Unproven);
            }
        }
    }
    crate::validate_mail_evidence_time(tx, evidence)
        .await
        .map_err(super::evidence_error)
        .map_err(|error| rejected("collected_claim_time_unproven", error))
}
