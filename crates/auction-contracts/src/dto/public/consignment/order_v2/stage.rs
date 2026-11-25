//! 订单阶段闭集，以及阶段组合与订单态的一致性规则。

use crate::dto::public::consignment::wire_v2::string_enum;

string_enum! {
    /// 订单种类：买家成交或原卖家直接撤回。
    pub enum OrderKindV2 {
        /// 买家成交。
        Sale => "sale",
        /// 原卖家撤回。
        Withdrawal => "withdrawal",
    }
}

string_enum! {
    /// 读取者在订单里的角色。
    pub enum OrderRoleV2 {
        /// 买家。
        Buyer => "buyer",
        /// 卖家。
        Seller => "seller",
    }
}

string_enum! {
    /// 订单对读取者的总体状态。
    pub enum OrderStateV2 {
        /// 等待买家付款。
        AwaitingPayment => "awaiting_payment",
        /// 已付款，等待原件确领。
        AwaitingHandoff => "awaiting_handoff",
        /// 原件已确领，等待卖款付出。
        AwaitingPayout => "awaiting_payout",
        /// 成交已取消，退款进行中。
        Refunding => "refunding",
        /// 撤回进行中。
        Withdrawing => "withdrawing",
        /// 已完成。
        Completed => "completed",
        /// 已取消。
        Cancelled => "cancelled",
        /// 证据矛盾或未知，等待核对。
        Reconcile => "reconcile",
    }
}

string_enum! {
    /// 原件或载体的交割阶段。
    pub enum DeliveryStageV2 {
        /// 尚未开始。
        NotStarted => "not_started",
        /// 已准备，未起写。
        Prepared => "prepared",
        /// 已起写，结果未证。
        InFlight => "in_flight",
        /// 已证发出。
        Sent => "sent",
        /// 已证确领。
        Claimed => "claimed",
        /// 已证退回。
        Returned => "returned",
        /// 已封闭。
        Closed => "closed",
        /// 结果未知。
        Unknown => "unknown",
    }
}

string_enum! {
    /// 买家付款阶段。
    pub enum PaymentStageV2 {
        /// 不适用（撤回订单）。
        NotApplicable => "not_applicable",
        /// 未付款。
        Unpaid => "unpaid",
        /// 实收已认证。
        Certified => "certified",
        /// 已封闭，未认证付款。
        Closed => "closed",
        /// 结果未知。
        Unknown => "unknown",
    }
}

string_enum! {
    /// 卖款与退款两条资金腿的阶段。
    pub enum LegStageV2 {
        /// 不适用。
        NotApplicable => "not_applicable",
        /// 被阻断。
        Blocked => "blocked",
        /// 可付出。
        Ready => "ready",
        /// 已预留。
        Reserved => "reserved",
        /// 已证发出。
        Sent => "sent",
        /// 已证确领。
        Claimed => "claimed",
        /// 已证退回。
        Returned => "returned",
        /// 已封闭。
        Closed => "closed",
        /// 结果未知。
        Unknown => "unknown",
    }
}

string_enum! {
    /// 订单上读取者可执行的动作。第 6 期恒为空，第 7 期放开时与校验同改。
    pub enum OrderActionV2 {
        /// 受限权利定位。
        Locate => "locate",
        /// 受限权利挑战。
        Challenge => "challenge",
        /// 受限权利验证。
        Verify => "verify",
        /// 受限权利领取。
        Claim => "claim",
    }
}

/// 阶段组合与订单态是否相容。`reconcile` 吸收矛盾组合，但不放宽适用性：
/// 成交订单一定有付款腿，撤回订单的三条资金腿一定不适用。
/// 领域投影 `project_order_stages` 与本函数各写一次，由共享夹具互核。
pub(in crate::dto) fn stages_match(
    kind: OrderKindV2,
    state: OrderStateV2,
    delivery: DeliveryStageV2,
    payment: PaymentStageV2,
    payout: LegStageV2,
    refund: LegStageV2,
) -> bool {
    use DeliveryStageV2 as D;
    use LegStageV2 as L;
    use OrderStateV2 as S;
    use PaymentStageV2 as P;

    let unknown = delivery == D::Unknown
        || payment == P::Unknown
        || payout == L::Unknown
        || refund == L::Unknown;
    if unknown && state != S::Reconcile {
        return false;
    }
    let before_claim = matches!(delivery, D::NotStarted | D::Prepared | D::InFlight | D::Sent);
    // 退款权转移的前提是原件已证退回或封闭（或从未起写）；prepared、in_flight、sent 仍可能被确领。
    let settled = matches!(delivery, D::NotStarted | D::Returned | D::Closed);
    let pre_payout = matches!(payout, L::NotApplicable | L::Blocked);
    let closed_leg = matches!(payout, L::NotApplicable | L::Closed);
    let active =
        |leg: L| matches!(leg, L::Blocked | L::Ready | L::Reserved | L::Sent | L::Returned);
    match kind {
        OrderKindV2::Sale => match state {
            S::Reconcile => payment != P::NotApplicable,
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
        OrderKindV2::Withdrawal => {
            if payment != P::NotApplicable
                || payout != L::NotApplicable
                || refund != L::NotApplicable
            {
                return false;
            }
            match state {
                S::Reconcile => true,
                S::Withdrawing => before_claim || delivery == D::Returned,
                S::Completed => delivery == D::Claimed,
                S::Cancelled => matches!(delivery, D::NotStarted | D::Returned | D::Closed),
                _ => false,
            }
        }
    }
}
