//! 订单阶段投影：把义务、履约、轮与资金的状态枚举收成订单态，矛盾一律落到 `reconcile`。
//!
//! 保护不变量 6：矛盾或未知的组合失败关闭，不静默修正成某个“看起来合理”的订单态。
//! 契约 crate 的 `OrderV2` 校验按订单态反向核对同一张表，两边由共享夹具互核。

/// 订单种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    /// 买家成交。
    Sale,
    /// 原卖家撤回。
    Withdrawal,
}

/// 合同的整体结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderOutcome {
    /// 仍在履行。
    Open,
    /// 已履行完毕。
    Completed,
    /// 已取消。
    Cancelled,
}

/// 订单态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderState {
    /// 等待买家付款。
    AwaitingPayment,
    /// 已付款，等待原件确领。
    AwaitingHandoff,
    /// 原件已确领，等待卖款付出。
    AwaitingPayout,
    /// 退款进行中。
    Refunding,
    /// 撤回进行中。
    Withdrawing,
    /// 已完成。
    Completed,
    /// 已取消。
    Cancelled,
    /// 证据矛盾或未知。
    Reconcile,
}

impl OrderState {
    /// 线协议码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingPayment => "awaiting_payment",
            Self::AwaitingHandoff => "awaiting_handoff",
            Self::AwaitingPayout => "awaiting_payout",
            Self::Refunding => "refunding",
            Self::Withdrawing => "withdrawing",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Reconcile => "reconcile",
        }
    }
}

/// 原件或载体的交割阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderDeliveryStage {
    /// 尚未开始。
    NotStarted,
    /// 已准备，未起写。
    Prepared,
    /// 已起写，结果未证。
    InFlight,
    /// 已证发出。
    Sent,
    /// 已证确领。
    Claimed,
    /// 已证退回。
    Returned,
    /// 已封闭。
    Closed,
    /// 结果未知。
    Unknown,
}

impl OrderDeliveryStage {
    /// 线协议码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Prepared => "prepared",
            Self::InFlight => "in_flight",
            Self::Sent => "sent",
            Self::Claimed => "claimed",
            Self::Returned => "returned",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }
}

/// 买家付款阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderPaymentStage {
    /// 不适用。
    NotApplicable,
    /// 未付款。
    Unpaid,
    /// 实收已认证。
    Certified,
    /// 已封闭，未认证付款。
    Closed,
    /// 结果未知。
    Unknown,
}

impl OrderPaymentStage {
    /// 线协议码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Unpaid => "unpaid",
            Self::Certified => "certified",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }
}

/// 卖款与退款两条资金腿的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderLegStage {
    /// 不适用。
    NotApplicable,
    /// 被阻断。
    Blocked,
    /// 可付出。
    Ready,
    /// 已预留。
    Reserved,
    /// 已证发出。
    Sent,
    /// 已证确领。
    Claimed,
    /// 已证退回。
    Returned,
    /// 已封闭。
    Closed,
    /// 结果未知。
    Unknown,
}

impl OrderLegStage {
    /// 线协议码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Blocked => "blocked",
            Self::Ready => "ready",
            Self::Reserved => "reserved",
            Self::Sent => "sent",
            Self::Claimed => "claimed",
            Self::Returned => "returned",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }
}

/// 存储侧为一张订单收集的状态枚举；不含金额。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderFacts {
    /// 订单种类。
    pub kind: OrderKind,
    /// 合同整体结局。
    pub outcome: OrderOutcome,
    /// 原件或载体的交割阶段。
    pub delivery: OrderDeliveryStage,
    /// 付款阶段。
    pub payment: OrderPaymentStage,
    /// 卖款阶段。
    pub payout: OrderLegStage,
    /// 退款阶段。
    pub refund: OrderLegStage,
}

/// 投影结果：订单态与规整后的四个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderStages {
    /// 订单态。
    pub order_state: OrderState,
    /// 交割阶段。
    pub delivery: OrderDeliveryStage,
    /// 付款阶段。
    pub payment: OrderPaymentStage,
    /// 卖款阶段。
    pub payout: OrderLegStage,
    /// 退款阶段。
    pub refund: OrderLegStage,
}

/// 把状态枚举投影成订单态。
///
/// 规整只做两件事：撤回订单的三条资金腿恒为不适用，成交订单的付款腿不得为不适用。
/// 事实里与之矛盾的取值会被规整值覆盖（输出须通过契约校验），但矛盾本身不被吞掉：
/// 订单态落到 `Reconcile`。
pub fn project_order_stages(facts: &OrderFacts) -> OrderStages {
    let mut stages = OrderStages {
        order_state: OrderState::Reconcile,
        delivery: facts.delivery,
        payment: facts.payment,
        payout: facts.payout,
        refund: facts.refund,
    };
    let mut contradiction = false;
    match facts.kind {
        OrderKind::Withdrawal => {
            contradiction = facts.payment != OrderPaymentStage::NotApplicable
                || facts.payout != OrderLegStage::NotApplicable
                || facts.refund != OrderLegStage::NotApplicable;
            stages.payment = OrderPaymentStage::NotApplicable;
            stages.payout = OrderLegStage::NotApplicable;
            stages.refund = OrderLegStage::NotApplicable;
        }
        OrderKind::Sale => {
            if facts.payment == OrderPaymentStage::NotApplicable {
                contradiction = true;
                stages.payment = OrderPaymentStage::Unknown;
            }
        }
    }
    if contradiction {
        return stages;
    }
    let candidate = candidate_state(facts);
    if stages_allow(facts.kind, candidate, &stages) {
        stages.order_state = candidate;
    }
    stages
}

/// 按结局与阶段挑出候选订单态；是否相容由 `stages_allow` 再核一次。
fn candidate_state(facts: &OrderFacts) -> OrderState {
    use OrderDeliveryStage as D;
    use OrderLegStage as L;
    use OrderPaymentStage as P;

    match (facts.kind, facts.outcome) {
        (_, OrderOutcome::Completed) => OrderState::Completed,
        (_, OrderOutcome::Cancelled) => OrderState::Cancelled,
        (OrderKind::Withdrawal, OrderOutcome::Open) => OrderState::Withdrawing,
        (OrderKind::Sale, OrderOutcome::Open) => {
            if !matches!(facts.refund, L::NotApplicable) {
                OrderState::Refunding
            } else if facts.payment == P::Unpaid {
                OrderState::AwaitingPayment
            } else if facts.payment == P::Certified && facts.delivery == D::Claimed {
                OrderState::AwaitingPayout
            } else if facts.payment == P::Certified {
                OrderState::AwaitingHandoff
            } else {
                OrderState::Reconcile
            }
        }
    }
}

/// 阶段组合是否与订单态相容。`Reconcile` 吸收矛盾，任何未知阶段只能落在 `Reconcile`。
fn stages_allow(kind: OrderKind, state: OrderState, stages: &OrderStages) -> bool {
    use OrderDeliveryStage as D;
    use OrderLegStage as L;
    use OrderPaymentStage as P;
    use OrderState as S;

    let (delivery, payment, payout, refund) =
        (stages.delivery, stages.payment, stages.payout, stages.refund);
    if delivery == D::Unknown
        || payment == P::Unknown
        || payout == L::Unknown
        || refund == L::Unknown
    {
        return state == S::Reconcile;
    }
    let before_claim = matches!(delivery, D::NotStarted | D::Prepared | D::InFlight | D::Sent);
    // 退款权转移的前提是原件已证退回或封闭（或从未起写）；prepared、in_flight、sent 仍可能被确领。
    let settled = matches!(delivery, D::NotStarted | D::Returned | D::Closed);
    let pre_payout = matches!(payout, L::NotApplicable | L::Blocked);
    let closed_leg = matches!(payout, L::NotApplicable | L::Closed);
    let active =
        |leg: L| matches!(leg, L::Blocked | L::Ready | L::Reserved | L::Sent | L::Returned);
    match kind {
        OrderKind::Sale => match state {
            S::Reconcile => true,
            S::AwaitingPayment => {
                payment == P::Unpaid && before_claim && pre_payout && refund == L::NotApplicable
            }
            S::AwaitingHandoff => {
                payment == P::Certified && before_claim && pre_payout && refund == L::NotApplicable
            }
            S::AwaitingPayout => {
                payment == P::Certified
                    && delivery == D::Claimed
                    && active(payout)
                    && refund == L::NotApplicable
            }
            S::Refunding => payment == P::Certified && settled && closed_leg && active(refund),
            S::Completed => {
                delivery == D::Claimed
                    && payment == P::Certified
                    && payout == L::Claimed
                    && refund == L::NotApplicable
            }
            S::Cancelled => {
                settled
                    && closed_leg
                    && ((payment == P::Closed && matches!(refund, L::NotApplicable | L::Closed))
                        || (payment == P::Certified && refund == L::Claimed))
            }
            S::Withdrawing => false,
        },
        OrderKind::Withdrawal => match state {
            S::Reconcile => true,
            S::Withdrawing => before_claim || delivery == D::Returned,
            S::Completed => delivery == D::Claimed,
            S::Cancelled => matches!(delivery, D::NotStarted | D::Returned | D::Closed),
            _ => false,
        },
    }
}
