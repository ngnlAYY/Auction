//! 履约进度的闭集公开状态与判定：把一件 lot 的事实按固定顺序映射为一个状态与附带字段。
//!
//! 判定只读事实、不写任何状态，首个命中的规则生效。顺序的理由：装备损失、成交撤销与退件会把
//! 付款或货款履约改写为满足或失败，必须先于普通的付款、交付与货款判定；recovery 的推进晚于
//! 履约（独立事务），所以付款是否完成以付款义务的履约阶段为准，逾期以付款期限为准，不看
//! recovery 的阶段名。阻断码只在撤销与退件阶段公开为“处理中”：付款提醒失败也会写它，但不影响
//! 付款收口。任何未知原值都让判定失败关闭，不猜成相邻的正常阶段。

use auction_domain::{SettlementObligationKind, SettlementObligationState, SettlementWorld};
use auction_time::Timestamp;

use super::facts::{PublicFulfillmentLot, PublicFulfillmentObligation, PublicFulfillmentRecovery};

/// 重新上架因赛季已结束而永久停止时写入 recovery 的阻断码。
const RELIST_SEASON_CLOSED: &str = "relist_season_closed";

/// 履约进度的闭集公开状态；wire 字符串一经发布不可改名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicFulfillmentStatus {
    /// 流拍，装备待寄回卖家。
    UnsoldReturnPending,
    /// 流拍，装备已寄回，等待卖家领取。
    UnsoldReturnSent,
    /// 流拍，卖家已取回装备。
    UnsoldReturned,
    /// 流拍，寄回处理中（失败或待人工核对）。
    UnsoldReturnOnHold,
    /// 已成交，付款邮件（COD）尚未寄出。
    PaymentMailPending,
    /// COD 已寄出，系统尚未确认收款。
    AwaitingPayment,
    /// 已过付款期限，系统仍未确认收款；只表示付款义务尚未收口，不断言买家没有付款。
    PaymentOverdue,
    /// 付款邮件处理中：投递失败、待人工核对，或期限已过仍无寄出证明。
    PaymentOnHold,
    /// COD 被买家退回，退件处理中；不指明资产：异世界的载体 COD 只有 Credits，没有装备。
    ReturnedByBuyer,
    /// 退件核对中。
    ReturnedOnHold,
    /// COD 被退回后待重新寄出。
    CodResendPending,
    /// COD 已重新寄出，系统尚未确认收款。
    CodResentAwaitingPayment,
    /// COD 退回次数已达上限，成交即将撤销。
    CodReturnLimitReached,
    /// 异世界：买家已付款，装备待交付。
    HandoffPending,
    /// 异世界：装备已寄出，等待买家领取。
    HandoffSent,
    /// 异世界：装备交付处理中。
    HandoffOnHold,
    /// 异世界：交付的装备被买家退回，处理中。
    HandoffReturnedByBuyer,
    /// 买家已付款（异世界已交付），货款待发给卖家。
    PayoutPending,
    /// 货款已寄出，等待卖家领取。
    PayoutSent,
    /// 货款发放处理中。
    PayoutOnHold,
    /// 卖家已领取货款，流程完成。
    Completed,
    /// 装备在寄出前损失，已人工处置；不表示买家已获补偿。
    EquipmentLost,
    /// 成交已撤销（COD 退回达上限），装备待寄回卖家。
    VoidedReturnPending,
    /// 成交已撤销，装备已寄回，等待卖家领取。
    VoidedReturnSent,
    /// 成交已撤销，卖家已取回装备。
    VoidedReturned,
    /// 成交已撤销，后续处理中。
    VoidedOnHold,
    /// 成交已撤销，待退款给买家。
    VoidedRefundPending,
    /// 成交已撤销，装备已重新上架。
    VoidedRelisted,
    /// 成交已撤销，等待重新上架。
    VoidedRelistPending,
    /// 成交已撤销，旧赛季装备不再上架。
    VoidedRelistUnavailable,
    /// 事实不足以判定（例如缺少结算明细或必需的义务）。
    Unknown,
}

impl PublicFulfillmentStatus {
    /// 返回冻结的 JSON 字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsoldReturnPending => "unsold_return_pending",
            Self::UnsoldReturnSent => "unsold_return_sent",
            Self::UnsoldReturned => "unsold_returned",
            Self::UnsoldReturnOnHold => "unsold_return_on_hold",
            Self::PaymentMailPending => "payment_mail_pending",
            Self::AwaitingPayment => "awaiting_payment",
            Self::PaymentOverdue => "payment_overdue",
            Self::PaymentOnHold => "payment_on_hold",
            Self::ReturnedByBuyer => "returned_by_buyer",
            Self::ReturnedOnHold => "returned_on_hold",
            Self::CodResendPending => "cod_resend_pending",
            Self::CodResentAwaitingPayment => "cod_resent_awaiting_payment",
            Self::CodReturnLimitReached => "cod_return_limit_reached",
            Self::HandoffPending => "handoff_pending",
            Self::HandoffSent => "handoff_sent",
            Self::HandoffOnHold => "handoff_on_hold",
            Self::HandoffReturnedByBuyer => "handoff_returned_by_buyer",
            Self::PayoutPending => "payout_pending",
            Self::PayoutSent => "payout_sent",
            Self::PayoutOnHold => "payout_on_hold",
            Self::Completed => "completed",
            Self::EquipmentLost => "equipment_lost",
            Self::VoidedReturnPending => "voided_return_pending",
            Self::VoidedReturnSent => "voided_return_sent",
            Self::VoidedReturned => "voided_returned",
            Self::VoidedOnHold => "voided_on_hold",
            Self::VoidedRefundPending => "voided_refund_pending",
            Self::VoidedRelisted => "voided_relisted",
            Self::VoidedRelistPending => "voided_relist_pending",
            Self::VoidedRelistUnavailable => "voided_relist_unavailable",
            Self::Unknown => "unknown",
        }
    }
}

/// 成交 lot 的公开成交事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFulfillmentSale {
    pub buyer_name: String,
    pub final_price: i64,
}

/// 成对公开的 COD 退回次数与该 recovery 冻结的上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicFulfillmentCodReturns {
    pub count: i32,
    pub limit: i32,
}

/// 一件 lot 的公开状态与按状态决定是否输出的附带字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFulfillmentView {
    pub status: PublicFulfillmentStatus,
    /// 成交 lot 的买家与成交价；流拍或缺结算明细时为 `None`。
    pub sale: Option<PublicFulfillmentSale>,
    /// 只在等待付款、已逾期与重寄后等待付款三种状态下给出当前付款期限。
    pub payment_deadline_at: Option<Timestamp>,
    /// 只在成交 lot 发生过 COD 退回时给出。
    pub cod_returns: Option<PublicFulfillmentCodReturns>,
}

/// 把一件 lot 的事实映射为公开状态与附带字段。
///
/// `world_id` 是场次世界，`read_at` 是同一读取快照内的时刻；付款期限恰好等于读取时刻即算逾期。
/// 原值不在闭集内、同种义务重复或成交事实自相矛盾时返回错误，由调用方让整页失败关闭。
pub fn public_fulfillment_view(
    lot: &PublicFulfillmentLot,
    world_id: &str,
    read_at: Timestamp,
) -> Result<PublicFulfillmentView, &'static str> {
    lot.validate()?;
    let world = SettlementWorld::from_name(world_id).ok_or("公开履约场次世界不在闭集内")?;
    let sold = match lot.lot_status.as_str() {
        "sold" => true,
        "unsold" => false,
        _ => return Err("公开履约 lot 状态不在闭集内"),
    };
    // 成交 lot 必有买家且成交价为正；流拍 lot 没有买家且结算金额全为零。
    let consistent = if sold {
        lot.buyer_name.is_some() && lot.final_price.is_none_or(|price| price > 0)
    } else {
        lot.buyer_name.is_none() && lot.final_price.is_none_or(|price| price == 0)
    };
    if !consistent {
        return Err("公开履约 lot 成交事实不一致");
    }
    let steps = Steps::parse(&lot.obligations)?;
    let recovery = lot.recovery.as_ref().map(Recovery::parse).transpose()?;
    let status = if sold {
        sold_status(lot, world, &steps, recovery.as_ref(), read_at)
    } else {
        unsold_status(lot, &steps)
    };

    let sale = match (sold, lot.final_price, lot.buyer_name.as_ref()) {
        (true, Some(final_price), Some(buyer_name)) => {
            Some(PublicFulfillmentSale { buyer_name: buyer_name.clone(), final_price })
        }
        _ => None,
    };
    let payment_deadline_at = matches!(
        status,
        PublicFulfillmentStatus::AwaitingPayment
            | PublicFulfillmentStatus::PaymentOverdue
            | PublicFulfillmentStatus::CodResentAwaitingPayment
    )
    .then(|| lot.recovery.as_ref().map(|recovery| recovery.payment_deadline_at))
    .flatten();
    let cod_returns =
        lot.recovery.as_ref().filter(|recovery| sold && recovery.cod_return_count > 0).map(
            |recovery| PublicFulfillmentCodReturns {
                count: recovery.cod_return_count,
                limit: recovery.cod_return_limit,
            },
        );
    Ok(PublicFulfillmentView { status, sale, payment_deadline_at, cod_returns })
}

/// 流拍 lot 只看寄回义务；缺结算明细或缺寄回义务时无法判定。
fn unsold_status(lot: &PublicFulfillmentLot, steps: &Steps) -> PublicFulfillmentStatus {
    if lot.final_price.is_none() {
        return PublicFulfillmentStatus::Unknown;
    }
    match steps.unsold_return.map(|step| step.phase) {
        None => PublicFulfillmentStatus::Unknown,
        Some(Phase::Done) => PublicFulfillmentStatus::UnsoldReturned,
        Some(Phase::Hold) => PublicFulfillmentStatus::UnsoldReturnOnHold,
        Some(Phase::Sent) => PublicFulfillmentStatus::UnsoldReturnSent,
        Some(Phase::Pending) => PublicFulfillmentStatus::UnsoldReturnPending,
    }
}

/// 成交 lot 依次判定：结算明细 → 装备损失 → 成交撤销 → 退件 → 付款 → 异世界交付 → 货款。
fn sold_status(
    lot: &PublicFulfillmentLot,
    world: SettlementWorld,
    steps: &Steps,
    recovery: Option<&Recovery<'_>>,
    read_at: Timestamp,
) -> PublicFulfillmentStatus {
    if lot.final_price.is_none() {
        return PublicFulfillmentStatus::Unknown;
    }
    if lot.equipment_lost {
        return PublicFulfillmentStatus::EquipmentLost;
    }
    if let Some(recovery) = recovery
        && let Some(status) = recovery_status(recovery, world, steps)
    {
        return status;
    }
    let Some(payment) = steps.payment(world) else {
        return PublicFulfillmentStatus::Unknown;
    };
    // recovery 缺失（历史成交）时不判逾期，也没有退回计数。
    let overdue = recovery.is_some_and(|recovery| recovery.facts.payment_deadline_at <= read_at);
    let returned = recovery.is_some_and(|recovery| recovery.facts.cod_return_count > 0);
    match payment.phase {
        Phase::Hold => return PublicFulfillmentStatus::PaymentOnHold,
        // 期限已过仍没有寄出证明：Worker 只延后、不提醒，转运维核对，对外显示处理中。
        Phase::Pending if overdue => return PublicFulfillmentStatus::PaymentOnHold,
        Phase::Sent if overdue => return PublicFulfillmentStatus::PaymentOverdue,
        Phase::Sent if returned => return PublicFulfillmentStatus::CodResentAwaitingPayment,
        Phase::Sent => return PublicFulfillmentStatus::AwaitingPayment,
        Phase::Pending if returned => return PublicFulfillmentStatus::CodResendPending,
        Phase::Pending => return PublicFulfillmentStatus::PaymentMailPending,
        Phase::Done => {}
    }
    if world == SettlementWorld::Isekai {
        match steps.handoff.map(|step| step.phase) {
            None => return PublicFulfillmentStatus::Unknown,
            Some(Phase::Hold) => return PublicFulfillmentStatus::HandoffOnHold,
            Some(Phase::Sent) => return PublicFulfillmentStatus::HandoffSent,
            Some(Phase::Pending) => return PublicFulfillmentStatus::HandoffPending,
            Some(Phase::Done) => {}
        }
    }
    match steps.payout {
        Some(Step { phase: Phase::Done, .. }) => PublicFulfillmentStatus::Completed,
        Some(Step { phase: Phase::Hold, .. }) => PublicFulfillmentStatus::PayoutOnHold,
        Some(Step { phase: Phase::Sent, .. }) => PublicFulfillmentStatus::PayoutSent,
        // 货款义务仍在等前置（blocked）时不应走到这里；无法解释的组合不猜成待发放。
        Some(Step { phase: Phase::Pending, state: SettlementObligationState::Ready }) => {
            PublicFulfillmentStatus::PayoutPending
        }
        _ => PublicFulfillmentStatus::Unknown,
    }
}

/// 成交撤销与退件阶段的状态；不在这两类时返回 `None`，交给付款与货款判定。
fn recovery_status(
    recovery: &Recovery<'_>,
    world: SettlementWorld,
    steps: &Steps,
) -> Option<PublicFulfillmentStatus> {
    if let Some(resolution) = recovery.resolution {
        return Some(voided_status(recovery, resolution, steps));
    }
    match recovery.stage {
        // 重挂与退款阶段必然带撤销结论；缺结论的组合无法解释。
        Stage::RelistPending | Stage::Refunding => Some(PublicFulfillmentStatus::Unknown),
        Stage::Reclaiming => Some(if recovery.blocked() {
            PublicFulfillmentStatus::ReturnedOnHold
        } else if world == SettlementWorld::Isekai
            && steps.payment(world).is_some_and(|payment| payment.phase == Phase::Done)
        {
            PublicFulfillmentStatus::HandoffReturnedByBuyer
        } else if world == SettlementWorld::Main
            && recovery.facts.cod_return_count >= recovery.facts.cod_return_limit
        {
            PublicFulfillmentStatus::CodReturnLimitReached
        } else {
            PublicFulfillmentStatus::ReturnedByBuyer
        }),
        Stage::AwaitingPayment
        | Stage::PaymentOverdue
        | Stage::DeliveringEquipment
        | Stage::Completed => None,
    }
}

/// 撤销结论首次落定即表示成交已撤销；按结论的后续流程给出撤销后的进度。
fn voided_status(
    recovery: &Recovery<'_>,
    resolution: Resolution,
    steps: &Steps,
) -> PublicFulfillmentStatus {
    match resolution {
        Resolution::ReturnedAfterCodReturns => {
            // 退回达上限后由撤拍寄回义务把装备还给卖家；义务尚未建立时视为待寄回。
            return match steps.withdrawn_return.map(|step| step.phase) {
                Some(Phase::Done) => PublicFulfillmentStatus::VoidedReturned,
                Some(Phase::Hold) => PublicFulfillmentStatus::VoidedOnHold,
                Some(Phase::Sent) => PublicFulfillmentStatus::VoidedReturnSent,
                Some(Phase::Pending) | None => PublicFulfillmentStatus::VoidedReturnPending,
            };
        }
        Resolution::EquipmentRecovered => match steps.refund.map(|step| step.phase) {
            Some(Phase::Hold) => return PublicFulfillmentStatus::VoidedOnHold,
            Some(Phase::Done) => {}
            Some(Phase::Pending | Phase::Sent) | None => {
                return PublicFulfillmentStatus::VoidedRefundPending;
            }
        },
        Resolution::Reclaimed | Resolution::Cancelled => {}
    }
    if recovery.facts.relisted {
        PublicFulfillmentStatus::VoidedRelisted
    } else if recovery.facts.blocking_code.as_deref() == Some(RELIST_SEASON_CLOSED) {
        PublicFulfillmentStatus::VoidedRelistUnavailable
    } else if recovery.blocked() {
        PublicFulfillmentStatus::VoidedOnHold
    } else {
        PublicFulfillmentStatus::VoidedRelistPending
    }
}

/// 单项义务的履约阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// 尚未寄出：履约行缺失、待准备、已准备，或已入队但投递仍在途。
    Pending,
    /// 已寄出待对方领取：履约已入队且投递成功。
    Sent,
    /// 已完成：履约已满足。
    Done,
    /// 处理中：履约失败或待人工核对，或已入队而投递失败、待人工核对。
    Hold,
}

/// 已解析的单项义务。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Step {
    phase: Phase,
    state: SettlementObligationState,
}

impl Step {
    fn parse(obligation: &PublicFulfillmentObligation) -> Result<Self, &'static str> {
        let state = SettlementObligationState::from_name(&obligation.business_state)
            .ok_or("公开履约义务业务状态不在闭集内")?;
        let delivery = obligation.delivery_status.as_deref().map(Delivery::parse).transpose()?;
        let phase = match obligation.execution_state.as_deref() {
            None | Some("pending" | "prepared") => Phase::Pending,
            Some("satisfied") => Phase::Done,
            Some("failed" | "manual_reconcile") => Phase::Hold,
            Some("enqueued") => match delivery {
                Some(Delivery::Succeeded) => Phase::Sent,
                Some(Delivery::Failed) => Phase::Hold,
                Some(Delivery::InFlight) | None => Phase::Pending,
            },
            Some(_) => return Err("公开履约执行状态不在闭集内"),
        };
        Ok(Self { phase, state })
    }
}

/// 投递状态的三类归并。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    InFlight,
    Succeeded,
    Failed,
}

impl Delivery {
    fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "prepared" | "pending" | "claimed" | "sending" => Ok(Self::InFlight),
            "succeeded" => Ok(Self::Succeeded),
            "failed" | "manual_reconcile" => Ok(Self::Failed),
            _ => Err("公开履约投递状态不在闭集内"),
        }
    }
}

/// 一件 lot 按种类归位的义务；每种至多一项。
#[derive(Debug, Default)]
struct Steps {
    equipment_cod: Option<Step>,
    payment_carrier: Option<Step>,
    handoff: Option<Step>,
    payout: Option<Step>,
    refund: Option<Step>,
    unsold_return: Option<Step>,
    withdrawn_return: Option<Step>,
}

impl Steps {
    fn parse(obligations: &[PublicFulfillmentObligation]) -> Result<Self, &'static str> {
        let mut steps = Self::default();
        for obligation in obligations {
            let kind = SettlementObligationKind::from_name(&obligation.kind)
                .ok_or("公开履约义务种类不在闭集内")?;
            let slot = match kind {
                SettlementObligationKind::WinnerEquipmentCod => &mut steps.equipment_cod,
                SettlementObligationKind::WinnerPaymentCarrierCod => &mut steps.payment_carrier,
                SettlementObligationKind::WinnerEquipmentHandoff => &mut steps.handoff,
                SettlementObligationKind::SellerPayout => &mut steps.payout,
                SettlementObligationKind::BuyerRefund => &mut steps.refund,
                SettlementObligationKind::UnsoldReturn => &mut steps.unsold_return,
                SettlementObligationKind::WithdrawnReturn => &mut steps.withdrawn_return,
            };
            if slot.replace(Step::parse(obligation)?).is_some() {
                return Err("公开履约同一 lot 的义务种类重复");
            }
        }
        Ok(steps)
    }

    /// 付款义务：主世界是装备 COD，异世界是主世界派发的载体 COD。
    fn payment(&self, world: SettlementWorld) -> Option<Step> {
        match world {
            SettlementWorld::Main => self.equipment_cod,
            SettlementWorld::Isekai => self.payment_carrier,
        }
    }
}

/// recovery 阶段闭集；领域 `RecoveryStage` 只建模了自动推进的四个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    AwaitingPayment,
    PaymentOverdue,
    DeliveringEquipment,
    Reclaiming,
    RelistPending,
    Refunding,
    Completed,
}

/// 撤销结论闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Reclaimed,
    Cancelled,
    EquipmentRecovered,
    ReturnedAfterCodReturns,
}

/// 已解析的 recovery。
#[derive(Debug)]
struct Recovery<'a> {
    stage: Stage,
    resolution: Option<Resolution>,
    facts: &'a PublicFulfillmentRecovery,
}

impl<'a> Recovery<'a> {
    fn parse(facts: &'a PublicFulfillmentRecovery) -> Result<Self, &'static str> {
        let stage = match facts.stage.as_str() {
            "awaiting_payment" => Stage::AwaitingPayment,
            "payment_overdue" => Stage::PaymentOverdue,
            "delivering_equipment" => Stage::DeliveringEquipment,
            "reclaiming" => Stage::Reclaiming,
            "relist_pending" => Stage::RelistPending,
            "refunding" => Stage::Refunding,
            "completed" => Stage::Completed,
            _ => return Err("公开履约 recovery 阶段不在闭集内"),
        };
        let resolution = match facts.resolution.as_deref() {
            None => None,
            Some("reclaimed") => Some(Resolution::Reclaimed),
            Some("cancelled") => Some(Resolution::Cancelled),
            Some("equipment_recovered") => Some(Resolution::EquipmentRecovered),
            Some("returned_after_three_cod_returns") => Some(Resolution::ReturnedAfterCodReturns),
            Some(_) => return Err("公开履约 recovery 撤销结论不在闭集内"),
        };
        Ok(Self { stage, resolution, facts })
    }

    /// recovery 是否被阻断或登记了待人工对账的投递。
    fn blocked(&self) -> bool {
        self.facts.blocking_code.is_some() || self.facts.manual_reconcile
    }
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};
    use uuid::Uuid;

    use super::{
        PublicFulfillmentCodReturns, PublicFulfillmentSale, PublicFulfillmentStatus,
        PublicFulfillmentView, public_fulfillment_view,
    };
    use crate::ports::public::fulfillment::facts::{
        PublicFulfillmentLot, PublicFulfillmentObligation, PublicFulfillmentRecovery,
    };

    fn now() -> Timestamp {
        Timestamp::from_timestamp(1_763_200_000, 0).expect("固定读取时刻")
    }

    fn obligation(
        kind: &str,
        business_state: &str,
        execution_state: Option<&str>,
        delivery_status: Option<&str>,
    ) -> PublicFulfillmentObligation {
        PublicFulfillmentObligation {
            kind: kind.to_owned(),
            business_state: business_state.to_owned(),
            execution_state: execution_state.map(str::to_owned),
            delivery_status: delivery_status.map(str::to_owned),
        }
    }

    fn recovery(stage: &str) -> PublicFulfillmentRecovery {
        PublicFulfillmentRecovery {
            stage: stage.to_owned(),
            resolution: None,
            payment_deadline_at: now() + Duration::days(7),
            cod_return_count: 0,
            cod_return_limit: 3,
            blocking_code: None,
            manual_reconcile: false,
            relisted: false,
        }
    }

    fn lot(
        lot_status: &str,
        obligations: Vec<PublicFulfillmentObligation>,
    ) -> PublicFulfillmentLot {
        let sold = lot_status == "sold";
        PublicFulfillmentLot {
            lot_id: Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2c),
            lot_number: 3,
            equipment_name: "Legendary Ethereal Staff".to_owned(),
            equipment_category: "Staff".to_owned(),
            seller_name: "seller".to_owned(),
            lot_status: lot_status.to_owned(),
            buyer_name: sold.then(|| "buyer".to_owned()),
            final_price: Some(if sold { 1_500_000 } else { 0 }),
            equipment_lost: false,
            recovery: sold.then(|| recovery("awaiting_payment")),
            obligations,
        }
    }

    /// 刚结算的主世界成交：COD 可执行但未寄出，货款等前置，退款为或有。
    fn sold_main() -> PublicFulfillmentLot {
        lot(
            "sold",
            vec![
                obligation("winner_equipment_cod", "ready", Some("pending"), None),
                obligation("seller_payout", "blocked", Some("pending"), None),
                obligation("buyer_refund", "contingent", Some("pending"), None),
            ],
        )
    }

    /// 刚结算的异世界成交：载体 COD 可执行，交付与货款等前置，退款为或有。
    fn sold_isekai() -> PublicFulfillmentLot {
        lot(
            "sold",
            vec![
                obligation("winner_payment_carrier_cod", "ready", Some("pending"), None),
                obligation("winner_equipment_handoff", "blocked", Some("pending"), None),
                obligation("seller_payout", "blocked", Some("pending"), None),
                obligation("buyer_refund", "contingent", Some("pending"), None),
            ],
        )
    }

    fn unsold() -> PublicFulfillmentLot {
        lot("unsold", vec![obligation("unsold_return", "ready", Some("pending"), None)])
    }

    /// 改写指定种类义务的业务状态与履约/投递原值。
    fn set(
        lot: &mut PublicFulfillmentLot,
        kind: &str,
        business_state: &str,
        execution_state: Option<&str>,
        delivery_status: Option<&str>,
    ) {
        lot.obligations.retain(|obligation| obligation.kind != kind);
        lot.obligations.push(obligation(kind, business_state, execution_state, delivery_status));
    }

    fn remove(lot: &mut PublicFulfillmentLot, kind: &str) {
        lot.obligations.retain(|obligation| obligation.kind != kind);
    }

    fn recovery_mut(lot: &mut PublicFulfillmentLot) -> &mut PublicFulfillmentRecovery {
        lot.recovery.as_mut().expect("成交 lot 带 recovery")
    }

    fn view(lot: &PublicFulfillmentLot, world: &str) -> PublicFulfillmentView {
        public_fulfillment_view(lot, world, now()).expect("事实可判定")
    }

    fn main_status(lot: &PublicFulfillmentLot) -> PublicFulfillmentStatus {
        view(lot, "main").status
    }

    fn isekai_status(lot: &PublicFulfillmentLot) -> PublicFulfillmentStatus {
        view(lot, "isekai").status
    }

    /// 主世界已付款：COD 满足，货款解锁。
    fn paid_main() -> PublicFulfillmentLot {
        let mut lot = sold_main();
        recovery_mut(&mut lot).stage = "completed".to_owned();
        set(&mut lot, "winner_equipment_cod", "ready", Some("satisfied"), Some("succeeded"));
        set(&mut lot, "seller_payout", "ready", Some("pending"), None);
        lot
    }

    /// 异世界已付款：载体满足，交付解锁。
    fn paid_isekai() -> PublicFulfillmentLot {
        let mut lot = sold_isekai();
        recovery_mut(&mut lot).stage = "delivering_equipment".to_owned();
        set(&mut lot, "winner_payment_carrier_cod", "ready", Some("satisfied"), Some("succeeded"));
        set(&mut lot, "winner_equipment_handoff", "ready", Some("pending"), None);
        lot
    }

    /// 主世界退件：退件已记录，计数加一。
    fn reclaiming_main(count: i32) -> PublicFulfillmentLot {
        let mut lot = sold_main();
        set(&mut lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("succeeded"));
        let recovery = recovery_mut(&mut lot);
        recovery.stage = "reclaiming".to_owned();
        recovery.cod_return_count = count;
        lot
    }

    /// COD 退回达上限后撤销：撤拍寄回义务把装备还给卖家，付款与货款履约失败收口。
    fn voided_after_returns() -> PublicFulfillmentLot {
        let mut lot = sold_main();
        set(&mut lot, "winner_equipment_cod", "ready", Some("failed"), Some("succeeded"));
        set(&mut lot, "seller_payout", "blocked", Some("failed"), None);
        let recovery = recovery_mut(&mut lot);
        recovery.stage = "completed".to_owned();
        recovery.resolution = Some("returned_after_three_cod_returns".to_owned());
        recovery.cod_return_count = 3;
        lot
    }

    /// Admin 确认收回（主世界）后的撤销：等待重挂。
    fn voided_reclaimed() -> PublicFulfillmentLot {
        let mut lot = reclaiming_main(1);
        let recovery = recovery_mut(&mut lot);
        recovery.stage = "relist_pending".to_owned();
        recovery.resolution = Some("reclaimed".to_owned());
        lot
    }

    /// 异世界确认装备已追回：退款激活。
    fn voided_equipment_recovered() -> PublicFulfillmentLot {
        let mut lot = paid_isekai();
        set(&mut lot, "buyer_refund", "ready", Some("pending"), None);
        let recovery = recovery_mut(&mut lot);
        recovery.stage = "refunding".to_owned();
        recovery.resolution = Some("equipment_recovered".to_owned());
        lot
    }

    /// 已发布的 wire 字符串逐一写死：交换两个变体的返回值、改名或漏改都会在这里失败，
    /// 不依赖实现本身生成期望值。
    const PUBLISHED_WIRE_STRINGS: [(PublicFulfillmentStatus, &str); 31] = [
        (PublicFulfillmentStatus::UnsoldReturnPending, "unsold_return_pending"),
        (PublicFulfillmentStatus::UnsoldReturnSent, "unsold_return_sent"),
        (PublicFulfillmentStatus::UnsoldReturned, "unsold_returned"),
        (PublicFulfillmentStatus::UnsoldReturnOnHold, "unsold_return_on_hold"),
        (PublicFulfillmentStatus::PaymentMailPending, "payment_mail_pending"),
        (PublicFulfillmentStatus::AwaitingPayment, "awaiting_payment"),
        (PublicFulfillmentStatus::PaymentOverdue, "payment_overdue"),
        (PublicFulfillmentStatus::PaymentOnHold, "payment_on_hold"),
        (PublicFulfillmentStatus::ReturnedByBuyer, "returned_by_buyer"),
        (PublicFulfillmentStatus::ReturnedOnHold, "returned_on_hold"),
        (PublicFulfillmentStatus::CodResendPending, "cod_resend_pending"),
        (PublicFulfillmentStatus::CodResentAwaitingPayment, "cod_resent_awaiting_payment"),
        (PublicFulfillmentStatus::CodReturnLimitReached, "cod_return_limit_reached"),
        (PublicFulfillmentStatus::HandoffPending, "handoff_pending"),
        (PublicFulfillmentStatus::HandoffSent, "handoff_sent"),
        (PublicFulfillmentStatus::HandoffOnHold, "handoff_on_hold"),
        (PublicFulfillmentStatus::HandoffReturnedByBuyer, "handoff_returned_by_buyer"),
        (PublicFulfillmentStatus::PayoutPending, "payout_pending"),
        (PublicFulfillmentStatus::PayoutSent, "payout_sent"),
        (PublicFulfillmentStatus::PayoutOnHold, "payout_on_hold"),
        (PublicFulfillmentStatus::Completed, "completed"),
        (PublicFulfillmentStatus::EquipmentLost, "equipment_lost"),
        (PublicFulfillmentStatus::VoidedReturnPending, "voided_return_pending"),
        (PublicFulfillmentStatus::VoidedReturnSent, "voided_return_sent"),
        (PublicFulfillmentStatus::VoidedReturned, "voided_returned"),
        (PublicFulfillmentStatus::VoidedOnHold, "voided_on_hold"),
        (PublicFulfillmentStatus::VoidedRefundPending, "voided_refund_pending"),
        (PublicFulfillmentStatus::VoidedRelisted, "voided_relisted"),
        (PublicFulfillmentStatus::VoidedRelistPending, "voided_relist_pending"),
        (PublicFulfillmentStatus::VoidedRelistUnavailable, "voided_relist_unavailable"),
        (PublicFulfillmentStatus::Unknown, "unknown"),
    ];

    #[test]
    fn wire_strings_match_the_published_table() {
        for (index, (status, wire)) in PUBLISHED_WIRE_STRINGS.into_iter().enumerate() {
            assert_eq!(status.as_str(), wire, "{status:?}");
            // 对照表里每个变体与字符串都只出现一次，31 行即覆盖整个闭集。
            assert!(
                PUBLISHED_WIRE_STRINGS[..index]
                    .iter()
                    .all(|(other, other_wire)| *other != status && *other_wire != wire),
                "{wire} 重复"
            );
        }
    }

    #[test]
    fn unsold_lots_follow_the_unsold_return() {
        let mut lot = unsold();
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::UnsoldReturnPending);
        set(&mut lot, "unsold_return", "ready", None, None);
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::UnsoldReturnPending, "履约行未建");
        set(&mut lot, "unsold_return", "ready", Some("enqueued"), Some("sending"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::UnsoldReturnPending, "投递在途");
        set(&mut lot, "unsold_return", "ready", Some("enqueued"), Some("succeeded"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::UnsoldReturnSent);
        set(&mut lot, "unsold_return", "ready", Some("satisfied"), Some("succeeded"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::UnsoldReturned);
        set(&mut lot, "unsold_return", "ready", Some("failed"), Some("failed"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::UnsoldReturnOnHold);
        set(&mut lot, "unsold_return", "ready", Some("enqueued"), Some("manual_reconcile"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::UnsoldReturnOnHold);
        // 流拍不公开买家与成交价，也没有付款字段。
        let shown = view(&lot, "isekai");
        assert_eq!((shown.sale, shown.payment_deadline_at, shown.cod_returns), (None, None, None));
    }

    #[test]
    fn missing_settlement_or_return_is_unknown() {
        let mut lot = unsold();
        remove(&mut lot, "unsold_return");
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::Unknown);
        let mut lot = unsold();
        lot.final_price = None;
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::Unknown);
        let mut lot = paid_main();
        lot.final_price = None;
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::Unknown);
        assert_eq!(shown.sale, None, "缺结算明细时不公开成交事实");
    }

    #[test]
    fn payment_states_follow_the_payment_fulfillment_and_deadline() {
        let mut lot = sold_main();
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::PaymentMailPending);
        assert_eq!(
            shown.sale,
            Some(PublicFulfillmentSale { buyer_name: "buyer".to_owned(), final_price: 1_500_000 })
        );
        assert_eq!(shown.payment_deadline_at, None, "尚未寄出时不公开期限");

        set(&mut lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("succeeded"));
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::AwaitingPayment);
        assert_eq!(shown.payment_deadline_at, Some(now() + Duration::days(7)));

        // 期限恰好等于读取时刻即算逾期；recovery 阶段名仍是 awaiting_payment 也不影响。
        recovery_mut(&mut lot).payment_deadline_at = now();
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::PaymentOverdue);
        assert_eq!(shown.payment_deadline_at, Some(now()));

        // 付款提醒失败写入的阻断码不公开，付款阶段照常判定。
        let recovery = recovery_mut(&mut lot);
        recovery.stage = "payment_overdue".to_owned();
        recovery.blocking_code = Some("recovery_delivery_failed".to_owned());
        recovery.manual_reconcile = true;
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PaymentOverdue);
    }

    #[test]
    fn payment_mail_problems_are_on_hold() {
        let mut lot = sold_main();
        set(&mut lot, "winner_equipment_cod", "ready", Some("failed"), Some("failed"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PaymentOnHold);
        set(&mut lot, "winner_equipment_cod", "ready", Some("manual_reconcile"), Some("sending"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PaymentOnHold);
        set(&mut lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("manual_reconcile"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PaymentOnHold);
        // 期限已过但付款邮件仍无寄出证明。
        set(&mut lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("pending"));
        recovery_mut(&mut lot).payment_deadline_at = now() - Duration::seconds(1);
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::PaymentOnHold);
        assert_eq!(shown.payment_deadline_at, None);
    }

    #[test]
    fn missing_recovery_never_reports_overdue_or_returns() {
        let mut lot = sold_main();
        set(&mut lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("succeeded"));
        lot.recovery = None;
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::AwaitingPayment);
        assert_eq!((shown.payment_deadline_at, shown.cod_returns), (None, None));
    }

    #[test]
    fn returned_cod_is_reclaimed_then_resent() {
        let lot = reclaiming_main(1);
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::ReturnedByBuyer);
        assert_eq!(shown.cod_returns, Some(PublicFulfillmentCodReturns { count: 1, limit: 3 }));

        // 自动重寄：原投递重置为待发送，recovery 回到等待付款，计数保留。
        let mut lot = reclaiming_main(1);
        recovery_mut(&mut lot).stage = "awaiting_payment".to_owned();
        set(&mut lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("pending"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::CodResendPending);
        set(&mut lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("succeeded"));
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::CodResentAwaitingPayment);
        assert_eq!(shown.payment_deadline_at, Some(now() + Duration::days(7)));
        assert_eq!(shown.cod_returns, Some(PublicFulfillmentCodReturns { count: 1, limit: 3 }));

        // 重寄后再逾期仍按期限判定。
        recovery_mut(&mut lot).payment_deadline_at = now() - Duration::hours(1);
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PaymentOverdue);
    }

    #[test]
    fn reclaiming_blocks_and_limits_take_precedence_in_order() {
        let mut lot = reclaiming_main(3);
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::CodReturnLimitReached);
        let shown = view(&lot, "main");
        assert_eq!(shown.cod_returns, Some(PublicFulfillmentCodReturns { count: 3, limit: 3 }));
        // 退件证据不符或回收动作失败时转人工：阻断优先于达上限。
        recovery_mut(&mut lot).blocking_code = Some("evidence_mismatch".to_owned());
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::ReturnedOnHold);
        let mut lot = reclaiming_main(1);
        recovery_mut(&mut lot).manual_reconcile = true;
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::ReturnedOnHold);
        // 达上限只适用于主世界的 COD 退回。
        let mut lot = sold_isekai();
        let recovery = recovery_mut(&mut lot);
        recovery.stage = "reclaiming".to_owned();
        recovery.cod_return_count = 3;
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::ReturnedByBuyer);
    }

    #[test]
    fn isekai_handoff_follows_payment() {
        let mut lot = sold_isekai();
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::PaymentMailPending);
        set(&mut lot, "winner_payment_carrier_cod", "ready", Some("enqueued"), Some("succeeded"));
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::AwaitingPayment);

        let mut lot = paid_isekai();
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::HandoffPending);
        set(&mut lot, "winner_equipment_handoff", "ready", Some("enqueued"), Some("succeeded"));
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::HandoffSent);
        set(&mut lot, "winner_equipment_handoff", "ready", Some("failed"), Some("failed"));
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::HandoffOnHold);
        remove(&mut lot, "winner_equipment_handoff");
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::Unknown);

        let mut lot = paid_isekai();
        set(&mut lot, "winner_equipment_handoff", "ready", Some("enqueued"), Some("succeeded"));
        recovery_mut(&mut lot).stage = "reclaiming".to_owned();
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::HandoffReturnedByBuyer);

        let mut lot = paid_isekai();
        set(&mut lot, "winner_equipment_handoff", "ready", Some("satisfied"), Some("succeeded"));
        set(&mut lot, "seller_payout", "ready", Some("pending"), None);
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::PayoutPending);
    }

    #[test]
    fn payout_states_follow_the_payout_fulfillment() {
        let mut lot = paid_main();
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PayoutPending);
        set(&mut lot, "seller_payout", "ready", Some("enqueued"), Some("claimed"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PayoutPending);
        set(&mut lot, "seller_payout", "ready", Some("enqueued"), Some("succeeded"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PayoutSent);
        set(&mut lot, "seller_payout", "ready", Some("manual_reconcile"), Some("manual_reconcile"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::PayoutOnHold);
        set(&mut lot, "seller_payout", "ready", Some("satisfied"), Some("succeeded"));
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::Completed);
        assert_eq!(shown.payment_deadline_at, None, "付款完成后不再公开期限");
        // 货款仍等前置或缺失时无法解释。
        set(&mut lot, "seller_payout", "blocked", Some("pending"), None);
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::Unknown);
        remove(&mut lot, "seller_payout");
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::Unknown);
        // 发生过退回的成交完成后仍成对公开退回次数。
        let mut lot = paid_main();
        recovery_mut(&mut lot).cod_return_count = 2;
        let shown = view(&lot, "main");
        assert_eq!(shown.cod_returns, Some(PublicFulfillmentCodReturns { count: 2, limit: 3 }));
    }

    #[test]
    fn missing_payment_obligation_is_unknown() {
        let mut lot = sold_main();
        remove(&mut lot, "winner_equipment_cod");
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::Unknown);
        // 异世界只认载体 COD 为付款义务。
        assert_eq!(isekai_status(&sold_main()), PublicFulfillmentStatus::Unknown);
    }

    #[test]
    fn equipment_loss_precedes_everything_else() {
        let mut lot = voided_reclaimed();
        lot.equipment_lost = true;
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::EquipmentLost);
        let mut lot = sold_main();
        set(&mut lot, "winner_equipment_cod", "ready", Some("satisfied"), Some("failed"));
        lot.equipment_lost = true;
        recovery_mut(&mut lot).stage = "completed".to_owned();
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::EquipmentLost);
    }

    #[test]
    fn voided_after_returns_follows_the_withdrawn_return() {
        let mut lot = voided_after_returns();
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::VoidedReturnPending, "寄回义务未建");
        assert_eq!(shown.cod_returns, Some(PublicFulfillmentCodReturns { count: 3, limit: 3 }));
        set(&mut lot, "withdrawn_return", "ready", None, None);
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::VoidedReturnPending);
        set(&mut lot, "withdrawn_return", "ready", Some("enqueued"), Some("succeeded"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::VoidedReturnSent);
        set(&mut lot, "withdrawn_return", "ready", Some("satisfied"), Some("succeeded"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::VoidedReturned);
        set(&mut lot, "withdrawn_return", "ready", Some("failed"), Some("failed"));
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::VoidedOnHold);
    }

    #[test]
    fn equipment_recovery_waits_for_the_refund() {
        let mut lot = voided_equipment_recovered();
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::VoidedRefundPending);
        set(&mut lot, "buyer_refund", "ready", Some("enqueued"), Some("succeeded"));
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::VoidedRefundPending);
        remove(&mut lot, "buyer_refund");
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::VoidedRefundPending);
        set(&mut lot, "buyer_refund", "ready", Some("failed"), Some("failed"));
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::VoidedOnHold);
        // 退款完成后沿用重挂的进度。
        set(&mut lot, "buyer_refund", "ready", Some("satisfied"), Some("succeeded"));
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::VoidedRelistPending);
        recovery_mut(&mut lot).relisted = true;
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::VoidedRelisted);
    }

    #[test]
    fn voided_sales_report_the_relist_progress() {
        let mut lot = voided_reclaimed();
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::VoidedRelistPending);
        recovery_mut(&mut lot).blocking_code = Some("relist_season_closed".to_owned());
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::VoidedRelistUnavailable);
        recovery_mut(&mut lot).blocking_code = Some("relist_failed".to_owned());
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::VoidedOnHold);
        recovery_mut(&mut lot).relisted = true;
        assert_eq!(main_status(&lot), PublicFulfillmentStatus::VoidedRelisted, "已重挂优先于阻断");
        // 异世界取消成交同样按重挂进度公开。
        let mut lot = paid_isekai();
        let recovery = recovery_mut(&mut lot);
        recovery.stage = "completed".to_owned();
        recovery.resolution = Some("cancelled".to_owned());
        assert_eq!(isekai_status(&lot), PublicFulfillmentStatus::VoidedRelistPending);
    }

    #[test]
    fn voiding_precedes_payment_and_reclaiming() {
        // 撤销后原付款履约可能仍是已寄出且逾期，不得显示成付款问题。
        let mut lot = voided_reclaimed();
        set(&mut lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("succeeded"));
        recovery_mut(&mut lot).payment_deadline_at = now() - Duration::days(1);
        let shown = view(&lot, "main");
        assert_eq!(shown.status, PublicFulfillmentStatus::VoidedRelistPending);
        assert_eq!(shown.payment_deadline_at, None);
        // 重挂与退款阶段缺撤销结论无法解释。
        for stage in ["relist_pending", "refunding"] {
            let mut lot = sold_main();
            recovery_mut(&mut lot).stage = stage.to_owned();
            assert_eq!(main_status(&lot), PublicFulfillmentStatus::Unknown, "{stage}");
        }
    }

    #[test]
    fn unknown_raw_values_fail_closed() {
        let cases: [fn(&mut PublicFulfillmentLot); 9] = [
            |lot| set(lot, "winner_equipment_cod", "ready", Some("cancelled"), None),
            |lot| set(lot, "winner_equipment_cod", "ready", Some("enqueued"), Some("returned")),
            |lot| set(lot, "winner_equipment_cod", "paused", Some("pending"), None),
            |lot| lot.obligations.push(obligation("seller_bonus", "ready", None, None)),
            |lot| lot.obligations.push(obligation("seller_payout", "ready", None, None)),
            |lot| recovery_mut(lot).stage = "relisting".to_owned(),
            |lot| recovery_mut(lot).resolution = Some("refunded".to_owned()),
            |lot| lot.lot_status = "withdrawn".to_owned(),
            |lot| set(lot, "seller_payout", "blocked", None, Some("pending")),
        ];
        for (index, change) in cases.into_iter().enumerate() {
            let mut lot = sold_main();
            change(&mut lot);
            assert!(public_fulfillment_view(&lot, "main", now()).is_err(), "case {index}");
        }
        assert!(public_fulfillment_view(&sold_main(), "persistent", now()).is_err());
    }

    #[test]
    fn inconsistent_sale_facts_fail_closed() {
        let mut lot = sold_main();
        lot.buyer_name = None;
        assert!(public_fulfillment_view(&lot, "main", now()).is_err());
        let mut lot = sold_main();
        lot.final_price = Some(0);
        assert!(public_fulfillment_view(&lot, "main", now()).is_err());
        let mut lot = unsold();
        lot.buyer_name = Some("buyer".to_owned());
        assert!(public_fulfillment_view(&lot, "main", now()).is_err());
        let mut lot = unsold();
        lot.final_price = Some(10);
        assert!(public_fulfillment_view(&lot, "main", now()).is_err());
        let mut lot = sold_main();
        recovery_mut(&mut lot).cod_return_count = 4;
        assert!(public_fulfillment_view(&lot, "main", now()).is_err(), "计数不能超过冻结上限");
    }
}
