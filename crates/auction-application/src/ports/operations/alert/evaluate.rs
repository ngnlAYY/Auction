//! operations alert 四来源阈值 evaluator。

use super::model::{
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
    OperationsAlertProcessSnapshot, OperationsAlertRecoverySnapshot, OperationsAlertSnapshot,
    OperationsAlertSourceObservation,
};
use super::signals::{OperationsAlertKind, OperationsAlertSignal, OperationsAlertSource};
use super::*;

/// 从四个 source 快照生成完整 signal 集合。
pub fn evaluate_snapshot(
    snapshot: &OperationsAlertSnapshot,
    policy: &OperationsAlertPolicy,
) -> Result<Vec<OperationsAlertSourceObservation>, OperationsAlertError> {
    policy.validate()?;
    let result = vec![
        evaluate_process(snapshot.process.as_ref())?,
        evaluate_delivery(snapshot.delivery, policy)?,
        evaluate_fulfillment(snapshot.fulfillment, policy)?,
        evaluate_recovery(snapshot.recovery.as_ref(), policy)?,
    ];
    let total = result.iter().map(|observation| observation.signals.len()).sum::<usize>();
    if total > policy.max_signals_per_scan {
        return Err(OperationsAlertError::Invalid);
    }
    Ok(result)
}

fn evaluate_process(
    snapshot: Option<&OperationsAlertProcessSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Process);
    };
    if snapshot.audit_failure_count < 0 || snapshot.processes.len() != 3 {
        return Err(OperationsAlertError::Invalid);
    }
    let mut seen = [false; 3];
    let mut signals = Vec::new();
    for process in &snapshot.processes {
        let index = match process.role.as_str() {
            "public" => 0,
            "admin" => 1,
            "worker" => 2,
            _ => return Err(OperationsAlertError::Invalid),
        };
        if seen[index] {
            return Err(OperationsAlertError::Invalid);
        }
        seen[index] = true;
        let kind = match process.status.as_str() {
            "ready" => None,
            "missing" => Some(OperationsAlertKind::ServiceMissing),
            "degraded" => Some(OperationsAlertKind::ServiceDegraded),
            _ => return Err(OperationsAlertError::Invalid),
        };
        if let Some(kind) = kind {
            signals.push(OperationsAlertSignal::new(
                kind,
                process.role.clone(),
                OperationsAlertSource::Process,
                1,
                0,
            )?);
        }
    }
    if !seen.into_iter().all(|value| value) {
        return Err(OperationsAlertError::Invalid);
    }
    if snapshot.audit_failure_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::AuditFailure,
            "audit",
            OperationsAlertSource::Process,
            snapshot.audit_failure_count,
            0,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Process, signals)
}

fn evaluate_delivery(
    snapshot: Option<OperationsAlertDeliverySnapshot>,
    policy: &OperationsAlertPolicy,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Delivery);
    };
    if snapshot.pending < 0
        || snapshot.failed < 0
        || snapshot.manual_reconcile < 0
        || snapshot.returned_mail_manual_reconcile < 0
        || snapshot.returned_mail_manual_reconcile > snapshot.manual_reconcile
        || snapshot.oldest_pending_age_seconds < 0
        || snapshot.oldest_manual_reconcile_age_seconds < 0
        || snapshot.oldest_returned_mail_age_seconds < 0
        || (snapshot.pending == 0 && snapshot.oldest_pending_age_seconds != 0)
        || (snapshot.manual_reconcile == 0 && snapshot.oldest_manual_reconcile_age_seconds != 0)
        || (snapshot.returned_mail_manual_reconcile == 0
            && snapshot.oldest_returned_mail_age_seconds != 0)
        || snapshot.last_update_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if snapshot.pending >= policy.delivery_pending_count_threshold
        || snapshot.oldest_pending_age_seconds
            >= policy.delivery_pending_age_threshold.as_secs() as i64
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::DeliveryPending,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.pending,
            snapshot.oldest_pending_age_seconds,
        )?);
    }
    // 退信仍计入总人工对账水位；独立 signal 只补充可执行提醒，不能替代总水位。
    if snapshot.returned_mail_manual_reconcile >= policy.delivery_manual_reconcile_count_threshold {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::DeliveryReturnedMail,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.returned_mail_manual_reconcile,
            snapshot.oldest_returned_mail_age_seconds,
        )?);
    }
    if snapshot.manual_reconcile >= policy.delivery_manual_reconcile_count_threshold {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::DeliveryManualReconcile,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.manual_reconcile,
            snapshot.oldest_manual_reconcile_age_seconds,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Delivery, signals)
}

fn evaluate_fulfillment(
    snapshot: Option<OperationsAlertFulfillmentSnapshot>,
    policy: &OperationsAlertPolicy,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Fulfillment);
    };
    let backlog =
        snapshot.pending.checked_add(snapshot.blocked).ok_or(OperationsAlertError::Invalid)?;
    if snapshot.pending < 0
        || snapshot.blocked < 0
        || snapshot.failed < 0
        || snapshot.manual_reconcile < 0
        || snapshot.oldest_backlog_age_seconds < 0
        || (backlog == 0 && snapshot.oldest_backlog_age_seconds != 0)
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if backlog >= policy.fulfillment_backlog_count_threshold
        || snapshot.oldest_backlog_age_seconds
            >= policy.fulfillment_backlog_age_threshold.as_secs() as i64
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentBacklog,
            "auction",
            OperationsAlertSource::Fulfillment,
            backlog,
            snapshot.oldest_backlog_age_seconds,
        )?);
    }
    if snapshot.failed > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentFailed,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.failed,
            0,
        )?);
    }
    if snapshot.manual_reconcile > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentManualReconcile,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.manual_reconcile,
            0,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Fulfillment, signals)
}

fn evaluate_recovery(
    snapshot: Option<&OperationsAlertRecoverySnapshot>,
    policy: &OperationsAlertPolicy,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Recovery);
    };
    if snapshot.payment_overdue_count < 0
        || snapshot.relist_pending_count < 0
        || snapshot.failed_action_count < 0
        || snapshot.manual_reconcile_count < 0
        || snapshot.oldest_overdue_age_seconds < 0
        || (snapshot.payment_overdue_count == 0 && snapshot.oldest_overdue_age_seconds != 0)
        || snapshot.worlds.is_empty()
        || snapshot.worlds.len() > 2
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if snapshot.payment_overdue_count >= policy.recovery_overdue_count_threshold
        || snapshot.oldest_overdue_age_seconds
            >= policy.recovery_overdue_age_threshold.as_secs() as i64
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryPaymentOverdue,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.payment_overdue_count,
            snapshot.oldest_overdue_age_seconds,
        )?);
    }
    if snapshot.relist_pending_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryRelistPending,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.relist_pending_count,
            0,
        )?);
    }
    if snapshot.failed_action_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryFailed,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.failed_action_count,
            0,
        )?);
    }
    if snapshot.manual_reconcile_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryManualReconcile,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.manual_reconcile_count,
            0,
        )?);
    }
    let mut worlds = Vec::new();
    for world in &snapshot.worlds {
        if !matches!(world.world.as_str(), "main" | "isekai")
            || worlds.iter().any(|value: &String| value == &world.world)
            || world.balance_credits < 0
            || world.evidence_age_seconds < 0
            || world.updated_at.timestamp_nanos_opt().is_none()
            || world.balance_observed_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
            || world
                .evidence_last_success_at
                .is_some_and(|value| value.timestamp_nanos_opt().is_none())
        {
            return Err(OperationsAlertError::Invalid);
        }
        worlds.push(world.world.clone());
        match world.balance_status.as_str() {
            "ok" => {}
            "low" => signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::BalanceLow,
                world.world.clone(),
                OperationsAlertSource::Recovery,
                world.balance_credits,
                0,
            )?),
            "unknown" | "stale" | "busy" | "unavailable" => {
                return OperationsAlertSourceObservation::unavailable(
                    OperationsAlertSource::Recovery,
                );
            }
            _ => return Err(OperationsAlertError::Invalid),
        }
        match world.evidence_status.as_str() {
            "fresh" => {}
            "stale" => signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::EvidenceStale,
                world.world.clone(),
                OperationsAlertSource::Recovery,
                1,
                world.evidence_age_seconds,
            )?),
            "unknown" | "unavailable" => {
                return OperationsAlertSourceObservation::unavailable(
                    OperationsAlertSource::Recovery,
                );
            }
            _ => return Err(OperationsAlertError::Invalid),
        }
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Recovery, signals)
}
