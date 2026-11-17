//! Recovery 状态、外部证据和结算义务校验。

use std::collections::BTreeMap;

use auction_application::recovery::{
    RecoveryCommandError, RecoveryConfirmationAction, RecoveryConfirmationCommand,
    recovery_confirmation_requires_blacklist,
};
use auction_time::Timestamp;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::recovery::commands::mapping::action_confirmation;
use crate::database::recovery::commands::mapping::confirmation_action_kind;
use crate::database::recovery::commands::mapping::map_sql_error;
use crate::database::recovery::commands::scope::{
    ActionRow, ObligationRow, RecoveryRow, RecoveryScope, lock_fulfillment_state,
};

pub(in crate::database::recovery::commands) fn validate_recovery_row(
    recovery: &RecoveryRow,
    scope: &RecoveryScope,
    command: &RecoveryConfirmationCommand,
) -> Result<(), RecoveryCommandError> {
    if recovery.id != command.recovery_id.as_uuid()
        || recovery.auction_id != scope.auction_id
        || recovery.lot_id != scope.lot_id
        || recovery.world_id != scope.world_id
        || recovery.equipment_id != scope.equipment_id
    {
        return Err(RecoveryCommandError::InvalidStoredResult);
    }
    if recovery.state_version != command.expected_state_version {
        return Err(RecoveryCommandError::StaleState);
    }
    if recovery.stage != "reclaiming"
        || recovery.resolution.is_some()
        || recovery.relisted_auction_id.is_some()
        || recovery.relisted_lot_id.is_some()
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    Ok(())
}

pub(in crate::database::recovery::commands) fn validate_actions<'a>(
    recovery: &RecoveryRow,
    actions: &'a [ActionRow],
    requested: RecoveryConfirmationAction,
) -> Result<&'a ActionRow, RecoveryCommandError> {
    let expected_kind = confirmation_action_kind(&recovery.world_id, requested)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let mut candidate = None;
    let mut blacklist_satisfied = false;
    let mut candidate_action = None;
    for action in actions {
        if action.kind == "buyer_blacklist" && action.state == "satisfied" {
            blacklist_satisfied = true;
        }
        if action.state != "satisfied" {
            continue;
        }
        let Some(candidate_value) = action_confirmation(&recovery.world_id, &action.kind) else {
            continue;
        };
        if candidate.is_some_and(|value| value != candidate_value) {
            return Err(RecoveryCommandError::NotConfirmable);
        }
        candidate = Some(candidate_value);
        candidate_action = Some(action);
    }

    let selected = actions
        .iter()
        .find(|action| action.kind == expected_kind)
        .ok_or(RecoveryCommandError::EvidenceUnresolved)?;
    if candidate != Some(requested)
        || selected.state != "satisfied"
        || selected.delivery_id.is_none()
        || selected.proof_kind.is_none()
        || selected.proof_reference.is_none()
        || selected.proof_digest.is_none()
        || selected.proof_observed_at.is_none()
        || candidate_action.map(|action| action.id) != Some(selected.id)
    {
        return Err(RecoveryCommandError::EvidenceUnresolved);
    }
    // 与 Admin 确认按钮同一规则：isekai 取消在退回次数未达冻结上限时不要求拉黑，达到上限与
    // main 收回仍要求既定拉黑已完成（冻结的处罚阈值不变，不变量 10）。
    if recovery_confirmation_requires_blacklist(
        requested,
        recovery.cod_return_count,
        recovery.cod_return_limit,
    ) && !blacklist_satisfied
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    Ok(selected)
}

pub(in crate::database::recovery::commands) fn validate_custody_evidence(
    recovery: &RecoveryRow,
    action: &ActionRow,
) -> Result<(), RecoveryCommandError> {
    if recovery.blocking_code.is_some()
        || recovery.manual_reconcile_delivery_id.is_some()
        || recovery.custody_proof_kind.as_deref() != Some("delivery_recovery_take_succeeded")
        || recovery.custody_proof_reference.is_none()
        || recovery.custody_proof_digest.is_none()
        || recovery.custody_proof_observed_at.is_none()
        || recovery.custody_proof_kind != action.proof_kind
        || recovery.custody_proof_reference != action.proof_reference
        || recovery.custody_proof_digest != action.proof_digest
        || recovery.custody_proof_observed_at != action.proof_observed_at
    {
        return Err(RecoveryCommandError::EvidenceUnresolved);
    }
    Ok(())
}

pub(in crate::database::recovery::commands) async fn validate_obligations(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    obligations: &BTreeMap<Uuid, ObligationRow>,
    action: RecoveryConfirmationAction,
) -> Result<(), RecoveryCommandError> {
    let payment = obligations
        .get(&recovery.payment_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let seller = obligations
        .get(&recovery.seller_payout_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    let refund = obligations
        .get(&recovery.buyer_refund_obligation_id)
        .ok_or(RecoveryCommandError::NotConfirmable)?;
    if seller.kind != "seller_payout"
        || seller.state != "blocked"
        || seller.amount <= 0
        || seller.recipient_game_identity.as_deref().is_none_or(str::is_empty)
        || refund.kind != "buyer_refund"
        || refund.state != "contingent"
        || refund.amount <= 0
        || refund.recipient_user_id.is_none()
    {
        return Err(RecoveryCommandError::NotConfirmable);
    }

    // 按同一 UUID 序锁全体 execution；完整执行但不收集未使用的行，不能按业务顺序反锁。
    let ids: Vec<Uuid> = obligations.keys().copied().collect();
    sqlx::query("SELECT obligation_id FROM public.settlement_obligation_fulfillments WHERE obligation_id=ANY($1) ORDER BY obligation_id FOR UPDATE")
        .bind(ids).execute(&mut **transaction).await.map_err(map_sql_error)?;
    let payment_state = lock_fulfillment_state(transaction, payment.id).await?;
    match action {
        RecoveryConfirmationAction::Reclaimed => {
            if recovery.world_id != "main"
                || payment.kind != "winner_equipment_cod"
                || payment.amount <= 0
                || payment.recipient_user_id.is_none()
                || payment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
        RecoveryConfirmationAction::Cancelled => {
            if recovery.world_id != "isekai"
                || payment.kind != "winner_payment_carrier_cod"
                || payment.amount <= 0
                || payment.recipient_user_id.is_none()
                || payment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
        RecoveryConfirmationAction::EquipmentRecovered => {
            let equipment_id =
                recovery.equipment_obligation_id.ok_or(RecoveryCommandError::NotConfirmable)?;
            let equipment =
                obligations.get(&equipment_id).ok_or(RecoveryCommandError::NotConfirmable)?;
            let equipment_state = lock_fulfillment_state(transaction, equipment.id).await?;
            if recovery.world_id != "isekai"
                || payment.kind != "winner_payment_carrier_cod"
                || payment_state.as_deref() != Some("satisfied")
                || equipment.kind != "winner_equipment_handoff"
                // 已付款会把 handoff 义务激活为 ready；外部交付仍须未满足且有精确取回证明。
                || equipment.state != "ready"
                || equipment_state.as_deref() == Some("satisfied")
            {
                return Err(RecoveryCommandError::NotConfirmable);
            }
        }
    }
    Ok(())
}

pub(in crate::database::recovery::commands) async fn activate_buyer_refund(
    transaction: &mut Transaction<'_, Postgres>,
    recovery: &RecoveryRow,
    now: Timestamp,
) -> Result<(), RecoveryCommandError> {
    let updated = sqlx::query(
        r#"UPDATE public.settlement_obligations
           SET business_state = 'ready', state_version = state_version + 1, updated_at = $1
           WHERE id = $2 AND auction_id = $3 AND lot_id = $4
             AND obligation_kind = 'buyer_refund' AND business_state = 'contingent'
           RETURNING id"#,
    )
    .bind(now)
    .bind(recovery.buyer_refund_obligation_id)
    .bind(recovery.auction_id)
    .bind(recovery.lot_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    if updated.is_none() {
        return Err(RecoveryCommandError::NotConfirmable);
    }
    // 激活后退款回到履约活动入口：同事务清除或有未激活标记（行已在 `validate_obligations`
    // 按 UUID 序锁定）。只写标记，不改执行状态与状态版本；迁移 000110 的延迟约束触发器在提交时
    // 拒绝漏清标记的激活。
    sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments
           SET dormant = false
           WHERE obligation_id = $1 AND dormant"#,
    )
    .bind(recovery.buyer_refund_obligation_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    Ok(())
}
