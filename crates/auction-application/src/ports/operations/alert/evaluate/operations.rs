//! 进程、生命周期与资产对账面的阈值评估。

use super::super::OperationsAlertError;
use super::super::model::{
    OperationsAlertLifecycleSnapshot, OperationsAlertProcessSnapshot,
    OperationsAlertReconciliationSnapshot, OperationsAlertSourceObservation,
};
use super::super::signals::{
    OPERATIONS_ALERT_LIFECYCLE_GRACE, OPERATIONS_ALERT_RECONCILIATION_STALE_AGE,
    OperationsAlertKind, OperationsAlertSignal, OperationsAlertSource, reached,
};

pub(super) fn evaluate_process(
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

pub(super) fn evaluate_lifecycle(
    snapshot: Option<OperationsAlertLifecycleSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Lifecycle);
    };
    if snapshot.phase_overdue_count < 0
        || snapshot.oldest_phase_overdue_age_seconds < 0
        || snapshot.proxy_expiry_overdue_count < 0
        || snapshot.oldest_proxy_overdue_age_seconds < 0
        || (snapshot.phase_overdue_count == 0 && snapshot.oldest_phase_overdue_age_seconds != 0)
        || (snapshot.proxy_expiry_overdue_count == 0
            && snapshot.oldest_proxy_overdue_age_seconds != 0)
        || snapshot.season_obligations_pending < 0
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    // 宽限期吸收 Worker 的调度抖动；只有到期后仍未推进才算“应进入 Final 而未进入”。
    if snapshot.phase_overdue_count > 0
        && reached(snapshot.oldest_phase_overdue_age_seconds, OPERATIONS_ALERT_LIFECYCLE_GRACE)
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::AuctionPhaseOverdue,
            "auction",
            OperationsAlertSource::Lifecycle,
            snapshot.phase_overdue_count,
            snapshot.oldest_phase_overdue_age_seconds,
        )?);
    }
    if snapshot.proxy_expiry_overdue_count > 0
        && reached(snapshot.oldest_proxy_overdue_age_seconds, OPERATIONS_ALERT_LIFECYCLE_GRACE)
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::ProxyExpiryOverdue,
            "auction",
            OperationsAlertSource::Lifecycle,
            snapshot.proxy_expiry_overdue_count,
            snapshot.oldest_proxy_overdue_age_seconds,
        )?);
    }
    if snapshot.season_obligations_pending > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::SeasonObligationsPending,
            "isekai",
            OperationsAlertSource::Lifecycle,
            snapshot.season_obligations_pending,
            0,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Lifecycle, signals)
}

/// 资产对账：报告缺失即来源不可用；有报告时按 world 报告差异与过期。
pub(super) fn evaluate_reconciliation(
    snapshot: Option<&OperationsAlertReconciliationSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(
            OperationsAlertSource::Reconciliation,
        );
    };
    let (Some(custody_age), Some(solvency_age)) =
        (snapshot.custody_report_age_seconds, snapshot.solvency_report_age_seconds)
    else {
        // 报告从未追加：没有可评估的差异事实，明确标记来源不可用而不是“无差异”。
        return OperationsAlertSourceObservation::unavailable(
            OperationsAlertSource::Reconciliation,
        );
    };
    if custody_age < 0
        || solvency_age < 0
        || snapshot.worlds.len() != 2
        || snapshot.worlds.iter().any(|world| {
            !matches!(world.world, "main" | "isekai")
                || world.custody_missing_count < 0
                || world.solvency_shortfall_credits < 0
        })
        || snapshot.worlds.iter().filter(|world| world.world == "main").count() != 1
        || snapshot.worlds.iter().filter(|world| world.world == "isekai").count() != 1
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if reached(custody_age, OPERATIONS_ALERT_RECONCILIATION_STALE_AGE) {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::ReconciliationStale,
            "custody",
            OperationsAlertSource::Reconciliation,
            1,
            custody_age,
        )?);
    }
    if reached(solvency_age, OPERATIONS_ALERT_RECONCILIATION_STALE_AGE) {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::ReconciliationStale,
            "solvency",
            OperationsAlertSource::Reconciliation,
            1,
            solvency_age,
        )?);
    }
    for world in &snapshot.worlds {
        if world.custody_missing_count > 0 {
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::CustodyDrift,
                world.world,
                OperationsAlertSource::Reconciliation,
                world.custody_missing_count,
                custody_age,
            )?);
        }
        if world.solvency_shortfall_credits > 0 {
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::SolvencyShortfall,
                world.world,
                OperationsAlertSource::Reconciliation,
                world.solvency_shortfall_credits,
                solvency_age,
            )?);
        }
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Reconciliation, signals)
}
