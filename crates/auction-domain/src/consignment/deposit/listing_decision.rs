//! 取得实物后的挂牌决策与代管控制结果。
//!
//! 优先级固定为：季界 → 撤回意愿 → 运营止售 → 资格 / 停用 → 额度。任何一项失败关闭时都
//! 不进入 `available`；撤回意愿与预停都不发明新状态，挂单留在 `deposit_accepted`。

use uuid::Uuid;

use crate::consignment::ListingState;
use crate::consignment::deposit::errors::DepositError;
use crate::consignment::errors::HeldOffMarketReason;
use crate::scheduling::SeasonState;

/// 取得全封后对挂单的唯一决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingDecision {
    /// 首次上架，进入唯一可购买态。
    Available,
    /// 留库下架，原因来自 D9 闭集。
    HeldOffMarket(HeldOffMarketReason),
    /// 已有撤回意愿；挂单留在 `deposit_accepted`，不占活动名额。
    WithdrawalPending,
    /// 处于预停窗口；暂不首次上架，等待季末关闭。
    SeasonPreclose,
    /// 旧季已关闭。
    SeasonClosed,
}

impl ListingDecision {
    /// 供审计与回执使用的稳定码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::HeldOffMarket(reason) => reason.as_str(),
            Self::WithdrawalPending => "withdrawal_pending",
            Self::SeasonPreclose => "season_preclose",
            Self::SeasonClosed => "season_closed",
        }
    }
}

/// 挂牌决策的锁内事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingDecisionFacts {
    /// 冻结季界与当前已知界取更严后的季态。
    pub season_state: SeasonState,
    /// 存在有效撤回意愿。
    pub withdrawal_requested: bool,
    /// 存在有效运营止售。
    pub operations_hold: bool,
    /// 卖家当前具备上架资格。
    pub seller_eligible: bool,
    /// 寄售服务当前启用。
    pub service_enabled: bool,
    /// 两世界活动名额仍有余量。
    pub quota_available: bool,
}

/// 应用固定优先级；调用方须在同一事务内用结果更新挂单与 position。
///
/// 不变量 6、10：季界与资格失败关闭；撤回意愿和预停不发明新状态，也不占用活动额度。
pub fn decide_listing_after_custody(facts: ListingDecisionFacts) -> ListingDecision {
    match facts.season_state {
        SeasonState::Closed => return ListingDecision::SeasonClosed,
        SeasonState::Preclose => return ListingDecision::SeasonPreclose,
        // 界点无法判定时不授权上架；留库复核，不占名额。
        SeasonState::Unknown => {
            return ListingDecision::HeldOffMarket(HeldOffMarketReason::Review);
        }
        SeasonState::NotSeasonal | SeasonState::Open => {}
    }
    if facts.withdrawal_requested {
        return ListingDecision::WithdrawalPending;
    }
    if facts.operations_hold {
        return ListingDecision::HeldOffMarket(HeldOffMarketReason::OperationsHold);
    }
    if !facts.seller_eligible {
        return ListingDecision::HeldOffMarket(HeldOffMarketReason::SellerIneligible);
    }
    if !facts.service_enabled {
        return ListingDecision::HeldOffMarket(HeldOffMarketReason::ServiceDisabled);
    }
    if !facts.quota_available {
        return ListingDecision::HeldOffMarket(HeldOffMarketReason::QuotaBlocked);
    }
    ListingDecision::Available
}

/// 卖家登记撤回意愿后的挂单结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalIntentOutcome {
    /// 尚未上架，留在 `deposit_accepted`；意愿等后期执行。
    RemainDepositAccepted,
    /// 已上架，转入 `withdrawal_pending` 并停止销售。
    MoveToWithdrawalPending,
    /// 已在下架态，保留原因并记录意愿。
    RemainHeldOffMarket,
    /// 已在撤回态，重放同一意愿。
    AlreadyPending,
}

/// 只有代管中的挂单可以登记撤回意愿；成交合同履行中与终态拒绝。
pub fn decide_withdrawal_intent(
    state: ListingState,
) -> Result<WithdrawalIntentOutcome, DepositError> {
    match state {
        ListingState::DepositAccepted => Ok(WithdrawalIntentOutcome::RemainDepositAccepted),
        ListingState::Available => Ok(WithdrawalIntentOutcome::MoveToWithdrawalPending),
        ListingState::HeldOffMarket => Ok(WithdrawalIntentOutcome::RemainHeldOffMarket),
        ListingState::WithdrawalPending => Ok(WithdrawalIntentOutcome::AlreadyPending),
        _ => Err(DepositError::ActionNotAllowed),
    }
}

/// Admin 对代管件设运营止售；未代管件继续走前三期关闭路径。
pub fn operations_hold_reason(state: ListingState) -> Result<HeldOffMarketReason, DepositError> {
    match state {
        ListingState::DepositAccepted | ListingState::Available | ListingState::HeldOffMarket => {
            Ok(HeldOffMarketReason::OperationsHold)
        }
        _ => Err(DepositError::ActionNotAllowed),
    }
}

/// 解除指定运营止售；解除后保持 `held_off_market/review`，不自动重新上架。
pub fn release_operations_hold_reason(
    state: ListingState,
    current_hold_id: Option<Uuid>,
    target_hold_id: Uuid,
) -> Result<HeldOffMarketReason, DepositError> {
    if state != ListingState::HeldOffMarket {
        return Err(DepositError::ActionNotAllowed);
    }
    if target_hold_id.is_nil() {
        return Err(DepositError::HoldMismatch);
    }
    match current_hold_id {
        Some(current) if current == target_hold_id => Ok(HeldOffMarketReason::Review),
        _ => Err(DepositError::HoldMismatch),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        ListingDecision, ListingDecisionFacts, WithdrawalIntentOutcome,
        decide_listing_after_custody, decide_withdrawal_intent, operations_hold_reason,
        release_operations_hold_reason,
    };
    use crate::consignment::ListingState;
    use crate::consignment::deposit::errors::DepositError;
    use crate::consignment::errors::HeldOffMarketReason;
    use crate::scheduling::SeasonState;

    fn facts() -> ListingDecisionFacts {
        ListingDecisionFacts {
            season_state: SeasonState::Open,
            withdrawal_requested: false,
            operations_hold: false,
            seller_eligible: true,
            service_enabled: true,
            quota_available: true,
        }
    }

    #[test]
    fn every_pair_observes_the_documented_priority() {
        let mut season_and_withdrawal = facts();
        season_and_withdrawal.season_state = SeasonState::Closed;
        season_and_withdrawal.withdrawal_requested = true;
        assert_eq!(
            decide_listing_after_custody(season_and_withdrawal),
            ListingDecision::SeasonClosed
        );

        let mut preclose_and_withdrawal = facts();
        preclose_and_withdrawal.season_state = SeasonState::Preclose;
        preclose_and_withdrawal.withdrawal_requested = true;
        assert_eq!(
            decide_listing_after_custody(preclose_and_withdrawal),
            ListingDecision::SeasonPreclose
        );

        let mut withdrawal_and_hold = facts();
        withdrawal_and_hold.withdrawal_requested = true;
        withdrawal_and_hold.operations_hold = true;
        assert_eq!(
            decide_listing_after_custody(withdrawal_and_hold),
            ListingDecision::WithdrawalPending
        );

        let mut hold_and_ineligible = facts();
        hold_and_ineligible.operations_hold = true;
        hold_and_ineligible.seller_eligible = false;
        assert_eq!(
            decide_listing_after_custody(hold_and_ineligible),
            ListingDecision::HeldOffMarket(HeldOffMarketReason::OperationsHold)
        );

        let mut ineligible_and_disabled = facts();
        ineligible_and_disabled.seller_eligible = false;
        ineligible_and_disabled.service_enabled = false;
        assert_eq!(
            decide_listing_after_custody(ineligible_and_disabled),
            ListingDecision::HeldOffMarket(HeldOffMarketReason::SellerIneligible)
        );

        let mut disabled_and_quota = facts();
        disabled_and_quota.service_enabled = false;
        disabled_and_quota.quota_available = false;
        assert_eq!(
            decide_listing_after_custody(disabled_and_quota),
            ListingDecision::HeldOffMarket(HeldOffMarketReason::ServiceDisabled)
        );
    }

    #[test]
    fn quota_permission_and_unknown_season_have_explicit_results() {
        let mut quota = facts();
        quota.quota_available = false;
        assert_eq!(
            decide_listing_after_custody(quota),
            ListingDecision::HeldOffMarket(HeldOffMarketReason::QuotaBlocked)
        );
        assert_eq!(decide_listing_after_custody(facts()), ListingDecision::Available);
        let mut unknown = facts();
        unknown.season_state = SeasonState::Unknown;
        assert_eq!(
            decide_listing_after_custody(unknown),
            ListingDecision::HeldOffMarket(HeldOffMarketReason::Review)
        );
    }

    #[test]
    fn withdrawal_and_hold_controls_follow_phase_four_states() {
        assert_eq!(
            decide_withdrawal_intent(ListingState::DepositAccepted),
            Ok(WithdrawalIntentOutcome::RemainDepositAccepted)
        );
        assert_eq!(
            decide_withdrawal_intent(ListingState::Available),
            Ok(WithdrawalIntentOutcome::MoveToWithdrawalPending)
        );
        assert_eq!(
            decide_withdrawal_intent(ListingState::SalePending),
            Err(DepositError::ActionNotAllowed)
        );
        assert_eq!(
            operations_hold_reason(ListingState::Available),
            Ok(HeldOffMarketReason::OperationsHold)
        );
        assert_eq!(
            operations_hold_reason(ListingState::AwaitingDeposit),
            Err(DepositError::ActionNotAllowed)
        );
        let hold = Uuid::from_u128(5);
        assert_eq!(
            release_operations_hold_reason(ListingState::HeldOffMarket, Some(hold), hold),
            Ok(HeldOffMarketReason::Review)
        );
        assert_eq!(
            release_operations_hold_reason(
                ListingState::HeldOffMarket,
                Some(hold),
                Uuid::from_u128(6)
            ),
            Err(DepositError::HoldMismatch)
        );
    }
}
