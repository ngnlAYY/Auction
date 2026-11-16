//! 在全部业务锁下重核当前用户、原义务、租约、依赖和完整 opaque 证明。

use auction_application::settlement::{
    FundingError, InternalEvidencePurpose, SettlementInternalCommand,
    SettlementInternalEvidenceSnapshot,
};
use auction_domain::{InternalSettlementAction, SettlementObligationKind};
use sqlx::{Postgres, Row};

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::{evidence_error, storage as sql_error};

pub(super) async fn validate(
    tx: &mut Transaction<'_, Postgres>,
    command: &SettlementInternalCommand,
    evidence: &SettlementInternalEvidenceSnapshot,
    collected: bool,
    external_refund: bool,
) -> Result<(), FundingError> {
    let claim = &command.claim;
    let rejected = |kind, error| {
        if collected {
            crate::database::settlement::funding::diagnostics::reject(kind, "claim_validate", error)
        } else {
            error
        }
    };
    let rejection = if !InternalSettlementAction::matches_frozen_shape(
        claim.kind,
        claim.world,
        claim.equipment_world,
        claim.amount,
        claim.carrier_credit,
    ) {
        Some("collected_claim_shape_invalid")
    } else if !evidence.validate() {
        Some("collected_claim_snapshot_invalid")
    } else if evidence.observed_from < claim.claimed_at {
        Some("collected_claim_observation_before_claim")
    } else if evidence.operation_id != command.command_id {
        Some("collected_claim_operation_mismatch")
    } else if evidence.identity.world != claim.world {
        Some("collected_claim_world_mismatch")
    } else if !matches!(
        &evidence.purpose,
        InternalEvidencePurpose::SettlementClaim {
            obligation_id,
            claim_token,
            claim_fencing,
            state_version
        } if *obligation_id == claim.obligation_id.as_uuid()
            && *claim_token == claim.claim_token
            && *claim_fencing == claim.claim_fencing
            && *state_version == claim.state_version
    ) {
        Some("collected_claim_binding_mismatch")
    } else if !collected && !external_refund && evidence.identity.player_name != claim.recipient {
        Some("collected_claim_recipient_mismatch")
    } else if external_refund && claim.kind != SettlementObligationKind::BuyerRefund {
        Some("collected_claim_refund_kind_invalid")
    } else {
        None
    };
    if let Some(kind) = rejection {
        return Err(rejected(kind, FundingError::Unproven));
    }
    let prepare_digest =
        claim.prepare_request_hash.map(auction_application::core::RequestHash::as_bytes);
    let activate_digest =
        claim.activate_request_hash.map(auction_application::core::RequestHash::as_bytes);
    let valid: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
               SELECT 1
               FROM public.settlement_obligations o
               JOIN public.settlement_obligation_fulfillments f ON f.obligation_id=o.id
               JOIN public.equipment e ON e.id=o.equipment_id
               JOIN public.lots l ON l.id=o.lot_id
                 AND l.auction_id=o.auction_id
               WHERE o.id=$1
                 AND o.auction_id=$2
                 AND o.lot_id=$3
                 AND o.origin_command_id=$4
                 AND o.obligation_kind=$5
                 AND o.amount=$6
                 AND o.carrier_credit=$7
                 AND o.equipment_id=$8
                 AND e.external_equipment_id=$9
                 AND e.world_id=$10
                 AND o.recipient_user_id IS NOT DISTINCT FROM $11
                 AND o.prerequisite_obligation_id IS NOT DISTINCT FROM $12
                 AND o.business_state='ready'
                 AND f.execution_state=$13
                 AND f.state_version=$14
                 AND f.claim_token=$15
                 AND f.claim_fencing=$16
                 AND f.claim_expires_at>clock_timestamp()
                 AND f.claim_expires_at=$23
                 AND f.dispatch_world=$17
                 AND f.delivery_operation='send'
                 AND f.delivery_id IS NOT DISTINCT FROM $18
                 AND (o.recipient_user_id IS NOT NULL OR o.recipient_game_identity=$19)
                 AND f.prepare_command_id IS NOT DISTINCT FROM $20
                 AND f.activate_command_id IS NOT DISTINCT FROM $21
                 AND o.submission_id IS NOT DISTINCT FROM $22
                 AND f.prepare_request_digest IS NOT DISTINCT FROM $24
                 AND f.activate_request_digest IS NOT DISTINCT FROM $25
                 AND f.proof_kind IS NULL
           )"#,
    )
    .bind(claim.obligation_id.as_uuid())
    .bind(claim.auction_id.as_uuid())
    .bind(claim.lot_id.as_uuid())
    .bind(claim.origin_command_id.as_uuid())
    .bind(claim.kind.as_str())
    .bind(claim.amount)
    .bind(i64::from(claim.carrier_credit))
    .bind(claim.equipment_id.as_uuid())
    .bind(&claim.external_equipment_id)
    .bind(claim.equipment_world.as_str())
    .bind(claim.recipient_user_id)
    .bind(claim.prerequisite_id.map(auction_domain::SettlementId::as_uuid))
    .bind(if collected { "enqueued" } else { "pending" })
    .bind(
        i64::try_from(claim.state_version)
            .map_err(|_| rejected("collected_claim_version_invalid", FundingError::Invalid))?,
    )
    .bind(claim.claim_token)
    .bind(claim.claim_fencing)
    .bind(claim.world.as_str())
    .bind(claim.delivery_id)
    .bind(&claim.recipient)
    .bind(claim.prepare_command_id.map(auction_domain::CommandId::as_uuid))
    .bind(claim.activate_command_id.map(auction_domain::CommandId::as_uuid))
    .bind(claim.submission_id.map(auction_domain::SubmissionId::as_uuid))
    .bind(claim.lease_until)
    .bind(prepare_digest.as_ref().map(<[u8; 32]>::as_slice))
    .bind(activate_digest.as_ref().map(<[u8; 32]>::as_slice))
    .fetch_one(&mut **tx)
    .await
    .map_err(sql_error)
    .map_err(|error| rejected("collected_claim_query_failed", error))?;
    if !valid {
        return Err(rejected("collected_claim_conflict", FundingError::Conflict));
    }
    if let Some(user_id) = claim.recipient_user_id {
        let user =
            sqlx::query("SELECT username,status,state_version FROM public.users WHERE id=$1")
                .bind(user_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(sql_error)
                .map_err(|error| rejected("collected_claim_user_query_failed", error))?;
        // 实收只验证 UUID 仍存在；地址由已发送 delivery 与 Sent 证明精确绑定。
        if !collected
            && (user.try_get::<String, _>("username").map_err(sql_error)? != claim.recipient
                || user.try_get::<String, _>("status").map_err(sql_error)? != "active"
                || command.recipient_version
                    != u64::try_from(user.try_get::<i64, _>("state_version").map_err(sql_error)?)
                        .ok())
        {
            return Err(FundingError::Conflict);
        }
    }
    if let Some(prerequisite) = claim.prerequisite_id {
        let satisfied: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(
                   SELECT 1
                   FROM public.settlement_obligation_fulfillments
                   WHERE obligation_id=$1
                     AND execution_state='satisfied'
                     AND proof_digest IS NOT NULL
               )"#,
        )
        .bind(prerequisite.as_uuid())
        .fetch_one(&mut **tx)
        .await
        .map_err(sql_error)
        .map_err(|error| rejected("collected_prerequisite_query_failed", error))?;
        if !satisfied {
            return Err(rejected("collected_prerequisite_unproven", FundingError::Unproven));
        }
    }
    if !collected {
        let action =
            InternalSettlementAction::for_kind(claim.kind).ok_or(FundingError::Unsupported)?;
        if claim.delivery_id.is_some()
            || claim.prepare_command_id.is_some()
            || claim.activate_command_id.is_some()
            || claim.prepare_request_hash.is_some()
            || claim.activate_request_hash.is_some()
        {
            return Err(FundingError::Conflict);
        }
        if action == InternalSettlementAction::KeepUnsold {
            let source: bool = sqlx::query_scalar(
                r#"SELECT EXISTS(
                       SELECT 1
                       FROM public.auction_submissions s
                       WHERE s.id=$1
                         AND s.source_kind IN ('mail','admin')
                         AND s.source_kind=$2
                         AND s.seller=$3
                         AND s.external_mail_id IS NOT DISTINCT FROM $4
                   )"#,
            )
            .bind(claim.submission_id.map(auction_domain::SubmissionId::as_uuid))
            .bind(&claim.submission_source_kind)
            .bind(&claim.recipient)
            .bind(&claim.submission_external_mail_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(sql_error)?;
            if !source {
                return Err(FundingError::Unproven);
            }
        }
        if action.requires_equipment() {
            let equipment = evidence.equipment.as_ref().ok_or(FundingError::Unproven)?;
            if equipment.equipment_id != claim.equipment_id
                || equipment.external_equipment_id != claim.external_equipment_id
                || equipment.binding.world != claim.equipment_world
            {
                return Err(FundingError::Unproven);
            }
        }
        if action == InternalSettlementAction::Handoff {
            // 原付款必须有本lane完整同账号证明；不凭旧无UID的collected字段推断当前账号实收。
            let paid: bool = sqlx::query_scalar(
                r#"SELECT EXISTS(
                       SELECT 1
                       FROM public.settlement_internal_commands
                       WHERE obligation_id=$1
                         AND action IN ('internal','collected')
                         AND uid=$2
                   )"#,
            )
            .bind(claim.prerequisite_id.map(auction_domain::SettlementId::as_uuid))
            .bind(&evidence.identity.authenticated_uid)
            .fetch_one(&mut **tx)
            .await
            .map_err(sql_error)?;
            if !paid {
                return Err(FundingError::Unproven);
            }
        }
        if action == InternalSettlementAction::Refund {
            // 原交付须仍为确认前的 enqueued，或已由确认事务以精确收口事实终结；任意其他 failed
            // 不能放行退款。
            let recovered: bool = sqlx::query_scalar(concat!(
                r#"SELECT EXISTS(
                       SELECT 1
                       FROM public.settlement_recoveries r
                       JOIN public.settlement_obligation_fulfillments p
                         ON p.obligation_id=r.payment_obligation_id
                       JOIN public.settlement_obligation_fulfillments h
                         ON h.obligation_id=r.equipment_obligation_id
                       JOIN public.deliveries hd ON hd.id=h.delivery_id
                       JOIN public.settlement_recovery_actions a ON a.recovery_id=r.id
                         AND a.kind='recover_equipment_take'
                       WHERE r.buyer_refund_obligation_id=$1
                         AND r.stage='refunding'
                         AND r.resolution='equipment_recovered'
                         AND r.custody_proof_digest IS NOT NULL
                         AND r.blocking_code IS NULL
                         AND a.execution_state='satisfied'
                         AND a.proof_digest=r.custody_proof_digest
                         AND (h.execution_state='enqueued' OR "#,
                crate::database::settlement::fulfillment::equipment_recovered_closure_sql!("r"),
                r#")
                         AND hd.status='succeeded'
                         AND (
                             hd.result_metadata->>'proof' IN (
                                 'sent_exact_payload_v1','sent_recovered_reference_v1'
                             )
                             OR public.delivery_has_frozen_sent_proof(
                                 hd.id,hd.result_metadata,hd.external_reference
                             )
                         )
                         AND hd.result_metadata->>'external_reference_proven'='true'
                         AND p.execution_state='satisfied'
                         AND p.proof_digest IS NOT NULL
                   )"#
            ))
            .bind(claim.obligation_id.as_uuid())
            .fetch_one(&mut **tx)
            .await
            .map_err(sql_error)?;
            if !recovered {
                return Err(FundingError::Unproven);
            }
        }
    }
    crate::validate_mail_evidence_time(tx, evidence)
        .await
        .map_err(evidence_error)
        .map_err(|error| rejected("collected_claim_time_unproven", error))
}
