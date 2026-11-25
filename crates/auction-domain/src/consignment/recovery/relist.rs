//! 重列判定：手动与自动共用同一组前置条件，自动模式不满足时留库而不是拒绝。
//!
//! 不变量 6、10：任何事实未知或未闭合都不得授权新销售代；自动开关只读决策时的冻结值。

use crate::consignment::deposit::DepositError;
use crate::consignment::errors::HeldOffMarketReason;
use crate::consignment::listing::ListingState;
use crate::consignment::recovery::disposition::RelistEligibility;
use crate::scheduling::SeasonState;

/// 重列方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelistMode {
    /// 卖家或 Admin 手动重列。
    Manual,
    /// 自动重列；`enabled_frozen` 是决策时读一次并冻结的 `auto_relist_enabled`。
    Auto { enabled_frozen: bool },
}

/// 重列的锁内事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelistFacts {
    /// 挂单状态。
    pub listing_state: ListingState,
    /// 当前下架原因。
    pub held_reason: Option<HeldOffMarketReason>,
    /// 存在独立有效的运营止售（决定 24）。
    pub active_hold: bool,
    /// 处置给出的再售资格。
    pub relist: RelistEligibility,
    /// 卖家具备挂牌资格。
    pub seller_eligible: bool,
    /// 活动额度仍有余量。
    pub quota_available: bool,
    /// 寄售服务启用。
    pub service_enabled: bool,
    /// 季态。
    pub season_state: SeasonState,
    /// 原件已精确收回并在库。
    pub original_in_custody: bool,
    /// 上代交割、退款与卖款全部封闭。
    pub predecessor_closed: bool,
    /// 存在未知派发。
    pub unknown_dispatch_open: bool,
    /// 服务 epoch 与冻结值一致（D9 §5.3）；过期 epoch 一律拒绝。
    pub epoch_current: bool,
    /// 重列方式。
    pub mode: RelistMode,
}

/// 重列决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelistDecision {
    /// 追加新销售代（当前代 + 1），用途为 `relist`。
    Relist {
        /// 新代用途码。
        next_purpose: &'static str,
    },
    /// 自动模式条件未满足：留库，原因如下。
    Keep {
        /// 保持的下架原因。
        reason: HeldOffMarketReason,
    },
    /// 手动模式或事实无效：拒绝。
    Reject(DepositError),
}

/// 按固定顺序验证：状态、资格、止售、季界、卖家、服务、额度、原件、上代、未知派发。
pub fn decide_relist(facts: &RelistFacts) -> RelistDecision {
    // 只有 held_off_market 且带原因的挂单可重列；自动模式下这也是装配错误，不留库。
    let Some(held_reason) = facts.held_reason else {
        return RelistDecision::Reject(DepositError::ListingNotActive);
    };
    if facts.listing_state != ListingState::HeldOffMarket {
        return RelistDecision::Reject(DepositError::ListingNotActive);
    }
    let auto = matches!(facts.mode, RelistMode::Auto { .. });
    let allowed = match facts.relist {
        RelistEligibility::Forbidden => false,
        RelistEligibility::ManualOnly => !auto,
        RelistEligibility::ManualOrAuto => true,
    };
    if !allowed {
        return RelistDecision::Reject(DepositError::ActionNotAllowed);
    }
    if let RelistMode::Auto { enabled_frozen: false } = facts.mode {
        return RelistDecision::Keep { reason: held_reason };
    }
    match first_blocker(facts) {
        None => RelistDecision::Relist { next_purpose: "relist" },
        Some((reason, _)) if auto => RelistDecision::Keep { reason },
        Some((_, error)) => RelistDecision::Reject(error),
    }
}

/// 第一个未满足的前置条件，同时给出自动模式的留库原因与手动模式的拒绝码。
fn first_blocker(facts: &RelistFacts) -> Option<(HeldOffMarketReason, DepositError)> {
    if facts.active_hold {
        return Some((HeldOffMarketReason::OperationsHold, DepositError::ActionNotAllowed));
    }
    if !facts.epoch_current {
        return Some((HeldOffMarketReason::Review, DepositError::ActionNotAllowed));
    }
    match facts.season_state {
        SeasonState::NotSeasonal | SeasonState::Open => {}
        SeasonState::Preclose | SeasonState::Closed => {
            return Some((HeldOffMarketReason::Review, DepositError::SeasonClosed));
        }
        SeasonState::Unknown => {
            return Some((HeldOffMarketReason::Review, DepositError::SeasonUnknown));
        }
    }
    if !facts.seller_eligible {
        return Some((HeldOffMarketReason::SellerIneligible, DepositError::SellerIneligible));
    }
    if !facts.service_enabled {
        return Some((HeldOffMarketReason::ServiceDisabled, DepositError::ActionNotAllowed));
    }
    if !facts.quota_available {
        return Some((HeldOffMarketReason::QuotaBlocked, DepositError::ActionNotAllowed));
    }
    if !facts.original_in_custody {
        return Some((HeldOffMarketReason::Review, DepositError::EvidenceUnproven));
    }
    if !facts.predecessor_closed || facts.unknown_dispatch_open {
        return Some((HeldOffMarketReason::Review, DepositError::EquipmentBusy));
    }
    None
}
