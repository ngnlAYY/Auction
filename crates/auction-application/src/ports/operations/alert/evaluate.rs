//! operations alert 四来源阈值 evaluator。

use super::model::{
    OperationsAlertCredentialSnapshot, OperationsAlertDeliverySnapshot,
    OperationsAlertForumSnapshot, OperationsAlertFulfillmentSnapshot,
    OperationsAlertLifecycleSnapshot, OperationsAlertMailSnapshot,
    OperationsAlertNotificationSnapshot, OperationsAlertPolicy, OperationsAlertProcessSnapshot,
    OperationsAlertReconciliationSnapshot, OperationsAlertRecoverySnapshot,
    OperationsAlertSnapshot, OperationsAlertSourceObservation,
};
use super::signals::{
    OPERATIONS_ALERT_ACCOUNT_BUSY_AGE, OPERATIONS_ALERT_LIFECYCLE_GRACE,
    OPERATIONS_ALERT_MAIL_POLL_STALL_AGE, OPERATIONS_ALERT_QUEUE_LATENCY_AGE,
    OPERATIONS_ALERT_RECONCILIATION_STALE_AGE, OperationsAlertKind, OperationsAlertSignal,
    OperationsAlertSource, reached,
};
use super::*;

/// 从全部来源快照生成完整 signal 集合；来源闭集见 `OperationsAlertSource::ALL`。
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
        evaluate_mail(snapshot.mail)?,
        evaluate_credential(snapshot.credential)?,
        evaluate_lifecycle(snapshot.lifecycle)?,
        evaluate_forum(snapshot.forum)?,
        evaluate_notification(snapshot.notification)?,
        evaluate_reconciliation(snapshot.reconciliation.as_ref())?,
    ];
    let total = result.iter().map(|observation| observation.signals.len()).sum::<usize>();
    if total > policy.max_signals_per_scan {
        return Err(OperationsAlertError::Invalid);
    }
    Ok(result)
}

/// W11.1–W11.3 资产对账：报告缺失即来源不可用；有报告时按 world 报告差异与过期。
fn evaluate_reconciliation(
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
        || snapshot.claimed < 0
        || snapshot.oldest_pending_age_seconds < 0
        || snapshot.oldest_manual_reconcile_age_seconds < 0
        || snapshot.oldest_returned_mail_age_seconds < 0
        || snapshot.oldest_claimed_age_seconds < 0
        || (snapshot.pending == 0 && snapshot.oldest_pending_age_seconds != 0)
        || (snapshot.claimed == 0 && snapshot.oldest_claimed_age_seconds != 0)
        || (snapshot.manual_reconcile == 0 && snapshot.oldest_manual_reconcile_age_seconds != 0)
        || (snapshot.returned_mail_manual_reconcile == 0
            && snapshot.oldest_returned_mail_age_seconds != 0)
        || snapshot.last_update_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    // 领取后长期没有推进属于队列延迟，与“从未领取”的 pending 积压是两个不同故障面。
    if snapshot.claimed > 0
        && reached(snapshot.oldest_claimed_age_seconds, OPERATIONS_ALERT_QUEUE_LATENCY_AGE)
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::QueueLatency,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.claimed,
            snapshot.oldest_claimed_age_seconds,
        )?);
    }
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
        || snapshot.payment_mail_unsent_count < 0
        || snapshot.relist_pending_count < 0
        || snapshot.failed_action_count < 0
        || snapshot.manual_reconcile_count < 0
        || snapshot.oldest_overdue_age_seconds < 0
        || snapshot.oldest_payment_mail_unsent_age_seconds < 0
        || (snapshot.payment_overdue_count == 0 && snapshot.oldest_overdue_age_seconds != 0)
        || (snapshot.payment_mail_unsent_count == 0
            && snapshot.oldest_payment_mail_unsent_age_seconds != 0)
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
    // 付款邮件未寄出但已逾期是独立的可处置故障：即使付款逾期总量尚未达到策略阈值，
    // 也必须让运维先核对付款投递，不能把 D9 只留在延后审计里。
    if snapshot.payment_mail_unsent_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryPaymentMailUnsent,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.payment_mail_unsent_count,
            snapshot.oldest_payment_mail_unsent_age_seconds,
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
    let mut available = true;
    for world in &snapshot.worlds {
        if !matches!(world.world.as_str(), "main" | "isekai")
            || worlds.iter().any(|value: &String| value == &world.world)
            || world.balance_credits < 0
            || world.evidence_age_seconds < 0
            || world.balance_stall_age_seconds < 0
            || world.updated_at.timestamp_nanos_opt().is_none()
            || world.balance_observed_at.is_some_and(|value| value.timestamp_nanos_opt().is_none())
            || world
                .evidence_last_success_at
                .is_some_and(|value| value.timestamp_nanos_opt().is_none())
        {
            return Err(OperationsAlertError::Invalid);
        }
        worlds.push(world.world.clone());
        if (world.balance_status == "not_applicable") != (world.evidence_status == "not_applicable")
        {
            return Err(OperationsAlertError::Invalid);
        }
        // 账号忙本身让来源不可用；持续超过阈值时再补一条可处置信号，
        // 让“一次瞬时忙”与“熔断/账号长期不可用”在仪表盘上可区分。
        if world.balance_status == "busy"
            && reached(world.balance_stall_age_seconds, OPERATIONS_ALERT_ACCOUNT_BUSY_AGE)
        {
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::AccountBusy,
                world.world.clone(),
                OperationsAlertSource::Recovery,
                1,
                world.balance_stall_age_seconds,
            )?);
        }
        match world.balance_status.as_str() {
            "ok" | "not_applicable" => {}
            "low" => signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::BalanceLow,
                world.world.clone(),
                OperationsAlertSource::Recovery,
                world.balance_credits,
                0,
            )?),
            "unknown" | "stale" | "busy" | "unavailable" => available = false,
            _ => return Err(OperationsAlertError::Invalid),
        }
        match world.evidence_status.as_str() {
            "fresh" | "not_applicable" => {}
            "stale" => signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::EvidenceStale,
                world.world.clone(),
                OperationsAlertSource::Recovery,
                1,
                world.evidence_age_seconds,
            )?),
            "unknown" | "unavailable" => available = false,
            _ => return Err(OperationsAlertError::Invalid),
        }
    }
    if available {
        OperationsAlertSourceObservation::available(OperationsAlertSource::Recovery, signals)
    } else {
        // 外部余额不可用不抹除数据库积压与 EvidenceStale，但保留来源不完整状态。
        let mut observation =
            OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Recovery)?;
        observation.signals.extend(signals);
        observation.validate()?;
        Ok(observation)
    }
}

fn evaluate_mail(
    snapshot: Option<OperationsAlertMailSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Mail);
    };
    if snapshot.last_poll_age_seconds < 0 || snapshot.consecutive_failures < 0 {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    // 显式暂停的邮箱已在读取层排除；这里只判断失败计数与水位超限。
    if snapshot.consecutive_failures > 0
        || reached(snapshot.last_poll_age_seconds, OPERATIONS_ALERT_MAIL_POLL_STALL_AGE)
    {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::MailPollStalled,
            "mail",
            OperationsAlertSource::Mail,
            snapshot.consecutive_failures,
            snapshot.last_poll_age_seconds,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Mail, signals)
}

fn evaluate_credential(
    snapshot: Option<OperationsAlertCredentialSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Credential);
    };
    if snapshot.invalid_mail_cookie < 0 || snapshot.invalid_external_api < 0 {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    for (scope, count) in [
        ("mail_cookie", snapshot.invalid_mail_cookie),
        ("external_api", snapshot.invalid_external_api),
    ] {
        if count > 0 {
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::CredentialInvalid,
                scope,
                OperationsAlertSource::Credential,
                count,
                0,
            )?);
        }
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Credential, signals)
}

fn evaluate_lifecycle(
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
    OperationsAlertSourceObservation::available(OperationsAlertSource::Lifecycle, signals)
}

fn evaluate_forum(
    snapshot: Option<OperationsAlertForumSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Forum);
    };
    if snapshot.sync_failed_count < 0
        || snapshot.oldest_sync_failed_age_seconds < 0
        || snapshot.message_failed_count < 0
        || snapshot.oldest_message_failed_age_seconds < 0
        || (snapshot.sync_failed_count == 0 && snapshot.oldest_sync_failed_age_seconds != 0)
        || (snapshot.message_failed_count == 0 && snapshot.oldest_message_failed_age_seconds != 0)
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    if snapshot.sync_failed_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::ForumSyncFailed,
            "forum",
            OperationsAlertSource::Forum,
            snapshot.sync_failed_count,
            snapshot.oldest_sync_failed_age_seconds,
        )?);
    }
    if snapshot.message_failed_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::ForumMessageFailed,
            "forum",
            OperationsAlertSource::Forum,
            snapshot.message_failed_count,
            snapshot.oldest_message_failed_age_seconds,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Forum, signals)
}

fn evaluate_notification(
    snapshot: Option<OperationsAlertNotificationSnapshot>,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Notification);
    };
    if snapshot.failed_count < 0
        || snapshot.oldest_failed_age_seconds < 0
        || snapshot.postage_insufficient_count < 0
        || snapshot.oldest_postage_age_seconds < 0
        || snapshot.postage_insufficient_count > snapshot.failed_count
        || (snapshot.failed_count == 0 && snapshot.oldest_failed_age_seconds != 0)
        || (snapshot.postage_insufficient_count == 0 && snapshot.oldest_postage_age_seconds != 0)
    {
        return Err(OperationsAlertError::Invalid);
    }
    let mut signals = Vec::new();
    // 邮资不足是失败的一个子集，仍计入总失败水位，不互相替代。
    if snapshot.postage_insufficient_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::NotificationPostageInsufficient,
            "notification",
            OperationsAlertSource::Notification,
            snapshot.postage_insufficient_count,
            snapshot.oldest_postage_age_seconds,
        )?);
    }
    if snapshot.failed_count > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::NotificationDeliveryFailed,
            "notification",
            OperationsAlertSource::Notification,
            snapshot.failed_count,
            snapshot.oldest_failed_age_seconds,
        )?);
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Notification, signals)
}
