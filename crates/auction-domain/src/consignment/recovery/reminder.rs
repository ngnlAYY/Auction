//! 付款提醒判定：到期且已证寄出、仍未付才提醒；派前再验，可能已派零补发。
//!
//! 不变量 4：外部写不自动重发；事实未知时不派（`Withhold`），不推测。

/// 判定发生的时点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderStage {
    /// 到期后建立提醒工作。
    Scheduling,
    /// 派出前的再验证。
    PreDispatch,
}

/// 提醒的锁内事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReminderFacts {
    /// 判定时点。
    pub stage: ReminderStage,
    /// 已到 `purchase_committed_at + frozen_payment_deadline`。
    pub due: bool,
    /// 原件寄出已证实。
    pub shipment_proven: bool,
    /// 该合同的提醒已建立（每合同一次）。
    pub already_queued: bool,
    /// 是否已付；`None` 为未知。
    pub paid: Option<bool>,
    /// 合同是否已取消；`None` 为未知。
    pub cancelled: Option<bool>,
    /// 提醒仍对应当前轮且 fence 一致。
    pub round_current: bool,
    /// 提醒可能已派出（含结果未知）。
    pub dispatch_possibly_sent: bool,
}

/// 提醒决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderDecision {
    /// 尚不需要提醒，或已建立过。
    NotDue,
    /// 建立或派出提醒。
    Queue,
    /// 已付，作废。
    VoidPaid,
    /// 合同已取消，作废。
    VoidCancelled,
    /// 轮或 fence 已变，作废。
    VoidRound,
    /// 可能已派出：不补发。
    NoResend,
    /// 付款或取消状态未知：不派，等事实补齐。
    Withhold,
}

/// 逾期本身不处罚；本函数只决定是否发提醒。
pub fn decide_reminder(facts: &ReminderFacts) -> ReminderDecision {
    if !facts.due || !facts.shipment_proven {
        return ReminderDecision::NotDue;
    }
    if facts.stage == ReminderStage::Scheduling && facts.already_queued {
        return ReminderDecision::NotDue;
    }
    if facts.stage == ReminderStage::PreDispatch && facts.dispatch_possibly_sent {
        return ReminderDecision::NoResend;
    }
    let (Some(paid), Some(cancelled)) = (facts.paid, facts.cancelled) else {
        return ReminderDecision::Withhold;
    };
    if cancelled {
        return ReminderDecision::VoidCancelled;
    }
    if paid {
        return ReminderDecision::VoidPaid;
    }
    if !facts.round_current {
        return ReminderDecision::VoidRound;
    }
    ReminderDecision::Queue
}
