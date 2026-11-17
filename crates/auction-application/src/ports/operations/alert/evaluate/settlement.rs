//! 结算面的投递、履约与恢复阈值评估。

use auction_domain::SettlementReturnTakeUnknownReason;

use super::super::OperationsAlertError;
use super::super::assessment::{assess_delivery, assess_fulfillment, assess_recovery};
use super::super::model::{
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
    OperationsAlertRecoverySnapshot, OperationsAlertSourceObservation,
};
use super::super::signals::{
    OPERATIONS_ALERT_ACCOUNT_BUSY_AGE, OperationsAlertKind, OperationsAlertSignal,
    OperationsAlertSource, reached,
};

pub(super) fn evaluate_delivery(
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
    let assessment = assess_delivery(snapshot, policy);
    let mut signals = Vec::new();
    // 领取后长期没有推进属于队列延迟，与“从未领取”的 pending 积压是两个不同故障面。
    if assessment.queue_latency {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::QueueLatency,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.claimed,
            snapshot.oldest_claimed_age_seconds,
        )?);
    }
    if assessment.pending {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::DeliveryPending,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.pending,
            snapshot.oldest_pending_age_seconds,
        )?);
    }
    // 退信仍计入总人工对账水位；独立 signal 只补充可执行提醒，不能替代总水位。
    if assessment.returned_mail {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::DeliveryReturnedMail,
            "delivery",
            OperationsAlertSource::Delivery,
            snapshot.returned_mail_manual_reconcile,
            snapshot.oldest_returned_mail_age_seconds,
        )?);
    }
    if assessment.manual_reconcile {
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

pub(super) fn evaluate_fulfillment(
    snapshot: Option<OperationsAlertFulfillmentSnapshot>,
    policy: &OperationsAlertPolicy,
) -> Result<OperationsAlertSourceObservation, OperationsAlertError> {
    let Some(snapshot) = snapshot else {
        return OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Fulfillment);
    };
    let actionable =
        snapshot.pending.checked_add(snapshot.blocked).ok_or(OperationsAlertError::Invalid)?;
    if snapshot.pending < 0
        || snapshot.blocked < 0
        || snapshot.failed < 0
        || snapshot.manual_reconcile < 0
        || snapshot.waiting_payment < 0
        || snapshot.waiting_prerequisite < 0
        || snapshot.oldest_backlog_age_seconds < 0
        || snapshot.oldest_prerequisite_wait_age_seconds < 0
        || (actionable == 0 && snapshot.oldest_backlog_age_seconds != 0)
        || (snapshot.waiting_prerequisite == 0
            && snapshot.oldest_prerequisite_wait_age_seconds != 0)
        // 读取失败只是等待领取的子集；没有等待领取时也不能有等待年龄。
        || snapshot.awaiting_collection < 0
        || snapshot.awaiting_collection_unreadable < 0
        || snapshot.awaiting_collection_unreadable > snapshot.awaiting_collection
        || snapshot.oldest_awaiting_collection_age_seconds < 0
        || (snapshot.awaiting_collection == 0
            && snapshot.oldest_awaiting_collection_age_seconds != 0)
        || snapshot.oldest_awaiting_collection_backlog_age_seconds < 0
        || (snapshot.awaiting_collection == 0
            && snapshot.oldest_awaiting_collection_backlog_age_seconds != 0)
        // 超期件数是等待领取的子集；有超期件时最久一笔必然达到本策略阈值，否则说明快照按
        // 别的阈值读取，失败关闭。
        || snapshot.collection_overdue < 0
        || snapshot.collection_overdue > snapshot.awaiting_collection
        || (snapshot.collection_overdue > 0
            && snapshot.oldest_awaiting_collection_age_seconds
                < policy.fulfillment_collection_overdue_age_threshold.as_secs() as i64)
        || snapshot.settlement_return_pending < 0
        || snapshot.oldest_settlement_return_pending_age_seconds < 0
        || (snapshot.settlement_return_pending == 0
            && snapshot.oldest_settlement_return_pending_age_seconds != 0)
        || snapshot.settlement_return_quarantined < 0
        || snapshot.oldest_settlement_return_quarantined_age_seconds < 0
        || (snapshot.settlement_return_quarantined == 0
            && snapshot.oldest_settlement_return_quarantined_age_seconds != 0)
        || snapshot.settlement_return_take_unknown.iter().any(|reason| {
            reason.count < 0
                || reason.oldest_age_seconds < 0
                || (reason.count == 0 && reason.oldest_age_seconds != 0)
        })
    {
        return Err(OperationsAlertError::Invalid);
    }
    let assessment = assess_fulfillment(snapshot, policy);
    // 等待前置义务超过策略年龄后才计入积压；等待买家付款的 COD 在付款期限内始终不算。
    let waiting_overdue = snapshot.waiting_prerequisite
        >= policy.fulfillment_backlog_count_threshold
        && snapshot.oldest_prerequisite_wait_age_seconds
            >= policy.fulfillment_backlog_age_threshold.as_secs() as i64;
    let backlog = if waiting_overdue {
        actionable
            .checked_add(snapshot.waiting_prerequisite)
            .ok_or(OperationsAlertError::Invalid)?
    } else {
        actionable
    };
    let oldest_backlog_age = if waiting_overdue {
        snapshot.oldest_backlog_age_seconds.max(snapshot.oldest_prerequisite_wait_age_seconds)
    } else {
        snapshot.oldest_backlog_age_seconds
    };
    let mut signals = Vec::new();
    if assessment.backlog {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentBacklog,
            "auction",
            OperationsAlertSource::Fulfillment,
            backlog,
            oldest_backlog_age,
        )?);
    }
    if assessment.failed {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentFailed,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.failed,
            0,
        )?);
    }
    if assessment.manual_reconcile {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentManualReconcile,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.manual_reconcile,
            0,
        )?);
    }
    // 长期未领取只提醒人工跟进：计数是达到阈值的件数（超期件数），年龄是最久一笔的寄出时长；
    // 告警不改变义务，也不触发重发或撤回。
    if assessment.collection_overdue {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::FulfillmentCollectionOverdue,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.collection_overdue,
            snapshot.oldest_awaiting_collection_age_seconds,
        )?);
    }
    // 结算退件按未结案案件计数，不设阈值：每件都需要 Admin 决定或核对；结案后由扫描关闭。
    if snapshot.settlement_return_pending > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::SettlementReturnPending,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.settlement_return_pending,
            snapshot.oldest_settlement_return_pending_age_seconds,
        )?);
    }
    if snapshot.settlement_return_quarantined > 0 {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::SettlementReturnQuarantined,
            "auction",
            OperationsAlertSource::Fulfillment,
            snapshot.settlement_return_quarantined,
            snapshot.oldest_settlement_return_quarantined_age_seconds,
        )?);
    }
    // Take 结果未知按原因各一个信号，作用域即原因名称，通知带出原因类别与处置指引。
    for (reason, watermark) in
        SettlementReturnTakeUnknownReason::ALL.iter().zip(snapshot.settlement_return_take_unknown)
    {
        if watermark.count > 0 {
            signals.push(OperationsAlertSignal::new(
                OperationsAlertKind::SettlementReturnTakeUnknown,
                reason.as_str(),
                OperationsAlertSource::Fulfillment,
                watermark.count,
                watermark.oldest_age_seconds,
            )?);
        }
    }
    OperationsAlertSourceObservation::available(OperationsAlertSource::Fulfillment, signals)
}

pub(super) fn evaluate_recovery(
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
    let assessment = assess_recovery(snapshot, policy);
    let mut signals = Vec::new();
    if assessment.payment_overdue {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryPaymentOverdue,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.payment_overdue_count,
            snapshot.oldest_overdue_age_seconds,
        )?);
    }
    // 付款邮件未寄出但已逾期是独立的可处置故障：即使付款逾期总量尚未达到策略阈值，
    // 也必须让运维先核对付款投递，不能把它只留在延后审计里。
    if assessment.payment_mail_unsent {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryPaymentMailUnsent,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.payment_mail_unsent_count,
            snapshot.oldest_payment_mail_unsent_age_seconds,
        )?);
    }
    if assessment.relist_pending {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryRelistPending,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.relist_pending_count,
            0,
        )?);
    }
    if assessment.failed_action {
        signals.push(OperationsAlertSignal::new(
            OperationsAlertKind::RecoveryFailed,
            "auction",
            OperationsAlertSource::Recovery,
            snapshot.failed_action_count,
            0,
        )?);
    }
    if assessment.manual_reconcile {
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
