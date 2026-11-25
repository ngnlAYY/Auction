//! D9 §5.1 六项授权矩阵：退回事实到“取消原买方权 / 退款 / 回库与本金 / 挂单 / 再售”的决定。
//!
//! “计拒收”与“处罚”不在本函数：前者归第 6 期 `return_fact`，后者归 §5.2 的阈值判定。
//! 不变量 6、10：矛盾、证据冲突、可交付性不明、相反付款可能已发，一律 `HoldForCase`，
//! 不自动取消、不退款、不再售；本函数只读事实，不修正输入。

use crate::SettlementWorld;
use crate::consignment::recovery::errors::RecoveryDomainError;

/// 被退回的是哪一条腿。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnedLeg {
    /// main 未付原件 COD。
    MainUnpaidOriginal,
    /// isekai 未付载体 COD（1 Credit，无装备）。
    IsekaiUnpaidCarrier,
    /// isekai 已付后的原件交接。
    IsekaiPaidHandoff,
    /// 买家退款。
    BuyerRefund,
    /// 卖款。
    SellerPayout,
    /// 撤回件。
    WithdrawalReturn,
}

/// 决定 26：可信 RTS 发生时原合同是否仍可交付；`Unclear` 只待判，不处罚。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryClosure {
    /// 发生于有效交割关闭界点之前，原合同仍可交付。
    StillDeliverable,
    /// 发生于关闭界点（含等号）之后。
    ClosedBeforeRts,
    /// 横跨界点或可交付性有实质争议。
    Unclear,
}

/// 退回处置的锁内事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnDispositionFacts {
    /// 被退回的腿。
    pub leg: ReturnedLeg,
    /// 合同所在世界。
    pub world: SettlementWorld,
    /// RTS 发生时的可交付性。
    pub closure: DeliveryClosure,
    /// 原件已精确 Take 消费。
    pub original_exactly_taken: bool,
    /// 载体本金 1 已精确 Take 消费。
    pub principal_exactly_taken: bool,
    /// main：本单有效未付轮数已达冻结上限。
    pub unpaid_returns_reached_limit: bool,
    /// 相反付款或卖款可能已发。
    pub opposite_payment_open: bool,
    /// 证据互相冲突。
    pub evidence_conflict: bool,
    /// 换季清空已发生。
    pub season_ended: bool,
    /// 买家原款已在账（锁内账本事实）；退 P 的必要输入，不在账时不得授权退款。
    pub original_payment_in_account: bool,
}

/// 对原买方权利的处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuyerRightEffect {
    /// 保留合同。
    Retain,
    /// main 达上限，按已证退回封闭原买方交割。
    CloseAtLimit,
    /// 首次即终止未付购买并封未启用下游。
    TerminateUnpaid,
    /// 同轮原件已退、无相反在途，转退款权。
    ConvertToRefund,
    /// 旧季交割已关，只封未付载体腿。
    TerminateCarrierLegOnly,
    /// 换季清空：关闭旧季实物交割，保留晚付监测。
    CloseOldSeason,
    /// 本腿不涉及买方权利。
    NotApplicable,
    /// 保全争议权，留案待判。
    HoldForCase,
}

/// 退款动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefundEffect {
    /// 无退款。
    None,
    /// 退回全额 P。
    RefundP,
    /// 只恢复原 refund 用途。
    RestoreRefundPurpose,
}

/// 载体本金动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalEffect {
    /// 无动作。
    None,
    /// 本金已精确收回，恢复原载体。
    RestoreOriginalCarrier,
    /// 只核销原本金 1。
    WriteOffOnly,
}

/// 挂单动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingEffect {
    /// 挂单不变。
    Unchanged,
    /// 转 `held_off_market/unpaid_recovered`。
    HeldUnpaidRecovered,
    /// 转 `held_off_market/paid_returned`。
    HeldPaidReturned,
}

/// 再售资格；真正放行还须 `decide_relist` 逐项验证。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelistEligibility {
    /// 禁止。
    Forbidden,
    /// 仅手动。
    ManualOnly,
    /// 手动或（开关开启时）自动。
    ManualOrAuto,
}

/// 处置决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnDispositionDecision {
    /// 原买方权利。
    pub buyer_right: BuyerRightEffect,
    /// 退款。
    pub refund: RefundEffect,
    /// 本金。
    pub principal: PrincipalEffect,
    /// 挂单。
    pub listing: ListingEffect,
    /// 再售资格。
    pub relist: RelistEligibility,
}

impl ReturnDispositionDecision {
    /// 留案：只保全争议权，零资金动作、挂单不动、禁止再售。
    const HOLD: Self = Self {
        buyer_right: BuyerRightEffect::HoldForCase,
        refund: RefundEffect::None,
        principal: PrincipalEffect::None,
        listing: ListingEffect::Unchanged,
        relist: RelistEligibility::Forbidden,
    };
}

/// 按 D9 §5.1 计算处置；事实矛盾或未知时留案而不是猜测。
pub fn decide_return_disposition(
    facts: &ReturnDispositionFacts,
) -> Result<ReturnDispositionDecision, RecoveryDomainError> {
    validate_shape(facts)?;
    if facts.evidence_conflict {
        return Ok(ReturnDispositionDecision::HOLD);
    }
    match facts.leg {
        ReturnedLeg::BuyerRefund => Ok(passive_leg(RefundEffect::RestoreRefundPurpose)),
        ReturnedLeg::SellerPayout | ReturnedLeg::WithdrawalReturn => {
            Ok(passive_leg(RefundEffect::None))
        }
        ReturnedLeg::MainUnpaidOriginal => Ok(main_unpaid_original(facts)),
        ReturnedLeg::IsekaiUnpaidCarrier => Ok(isekai_unpaid_carrier(facts)),
        ReturnedLeg::IsekaiPaidHandoff => Ok(isekai_paid_handoff(facts)),
    }
}

/// 腿与世界、换季事实的一致性；不一致是装配错误，直接失败。
fn validate_shape(facts: &ReturnDispositionFacts) -> Result<(), RecoveryDomainError> {
    let isekai_leg =
        matches!(facts.leg, ReturnedLeg::IsekaiUnpaidCarrier | ReturnedLeg::IsekaiPaidHandoff);
    let main_leg = facts.leg == ReturnedLeg::MainUnpaidOriginal;
    match facts.world {
        SettlementWorld::Main if isekai_leg => Err(RecoveryDomainError::LegWorldMismatch),
        SettlementWorld::Isekai if main_leg => Err(RecoveryDomainError::LegWorldMismatch),
        SettlementWorld::Main if facts.season_ended => Err(RecoveryDomainError::SeasonEndedOnMain),
        _ => Ok(()),
    }
}

/// 退款、卖款与撤回件被退：原受益权继续，无处罚，不得当作未付收回重列。
fn passive_leg(refund: RefundEffect) -> ReturnDispositionDecision {
    ReturnDispositionDecision {
        buyer_right: BuyerRightEffect::NotApplicable,
        refund,
        principal: PrincipalEffect::None,
        listing: ListingEffect::Unchanged,
        relist: RelistEligibility::Forbidden,
    }
}

fn main_unpaid_original(facts: &ReturnDispositionFacts) -> ReturnDispositionDecision {
    // main 不受季界约束（换季事实在 validate_shape 已拒绝），但可交付性不明或
    // 相反付款可能已发都不能判定，留案，不得再售。
    if facts.closure != DeliveryClosure::StillDeliverable || facts.opposite_payment_open {
        return ReturnDispositionDecision::HOLD;
    }
    if facts.unpaid_returns_reached_limit {
        // 达上限即封闭原买方交割，与原件是否已收回无关；再售还须原件精确收回。
        let taken = facts.original_exactly_taken;
        return ReturnDispositionDecision {
            buyer_right: BuyerRightEffect::CloseAtLimit,
            refund: RefundEffect::None,
            principal: PrincipalEffect::None,
            listing: if taken {
                ListingEffect::HeldUnpaidRecovered
            } else {
                ListingEffect::Unchanged
            },
            relist: if taken {
                RelistEligibility::ManualOrAuto
            } else {
                RelistEligibility::Forbidden
            },
        };
    }
    // 未达上限：保留合同，精确收回原件后才可新轮重寄；不触发再售。
    ReturnDispositionDecision {
        buyer_right: BuyerRightEffect::Retain,
        refund: RefundEffect::None,
        principal: PrincipalEffect::None,
        listing: ListingEffect::Unchanged,
        relist: RelistEligibility::Forbidden,
    }
}

fn isekai_unpaid_carrier(facts: &ReturnDispositionFacts) -> ReturnDispositionDecision {
    if facts.opposite_payment_open {
        return ReturnDispositionDecision::HOLD;
    }
    match facts.closure {
        DeliveryClosure::Unclear => ReturnDispositionDecision::HOLD,
        // 旧季交割已关：只封载体腿，本金精确收回后才核销，旧季禁止重列。
        DeliveryClosure::ClosedBeforeRts => ReturnDispositionDecision {
            buyer_right: BuyerRightEffect::TerminateCarrierLegOnly,
            refund: RefundEffect::None,
            principal: if facts.principal_exactly_taken {
                PrincipalEffect::WriteOffOnly
            } else {
                PrincipalEffect::None
            },
            listing: ListingEffect::Unchanged,
            relist: RelistEligibility::Forbidden,
        },
        DeliveryClosure::StillDeliverable => {
            // 换季清空与“仍可交付”互相矛盾，留案。
            if facts.season_ended {
                return ReturnDispositionDecision::HOLD;
            }
            // 本金与原物分别验证；两者都精确收回才放行挂单与再售。
            let both = facts.original_exactly_taken && facts.principal_exactly_taken;
            ReturnDispositionDecision {
                buyer_right: BuyerRightEffect::TerminateUnpaid,
                refund: RefundEffect::None,
                principal: if facts.principal_exactly_taken {
                    PrincipalEffect::RestoreOriginalCarrier
                } else {
                    PrincipalEffect::None
                },
                listing: if both {
                    ListingEffect::HeldUnpaidRecovered
                } else {
                    ListingEffect::Unchanged
                },
                relist: if both {
                    RelistEligibility::ManualOrAuto
                } else {
                    RelistEligibility::Forbidden
                },
            }
        }
    }
}

fn isekai_paid_handoff(facts: &ReturnDispositionFacts) -> ReturnDispositionDecision {
    // 交付未知先保全 P：相反卖款可能已发时不转退款。
    if facts.opposite_payment_open || facts.closure == DeliveryClosure::Unclear {
        return ReturnDispositionDecision::HOLD;
    }
    // 原款不在账：没有可退的 P，失败关闭留案（D9 §5.1、决定 21、22）。
    if !facts.original_payment_in_account && (facts.season_ended || facts.original_exactly_taken) {
        return ReturnDispositionDecision::HOLD;
    }
    if facts.season_ended {
        // 换季清空：已付未交付全退 P，不造回库，旧季禁止再售。
        return ReturnDispositionDecision {
            buyer_right: BuyerRightEffect::CloseOldSeason,
            refund: RefundEffect::RefundP,
            principal: PrincipalEffect::None,
            listing: ListingEffect::Unchanged,
            relist: RelistEligibility::Forbidden,
        };
    }
    if !facts.original_exactly_taken {
        // 原件未精确收回前不转退款，保留合同等待。
        return ReturnDispositionDecision {
            buyer_right: BuyerRightEffect::Retain,
            refund: RefundEffect::None,
            principal: PrincipalEffect::None,
            listing: ListingEffect::Unchanged,
            relist: RelistEligibility::Forbidden,
        };
    }
    ReturnDispositionDecision {
        buyer_right: BuyerRightEffect::ConvertToRefund,
        refund: RefundEffect::RefundP,
        principal: PrincipalEffect::None,
        listing: ListingEffect::HeldPaidReturned,
        relist: RelistEligibility::ManualOnly,
    }
}
