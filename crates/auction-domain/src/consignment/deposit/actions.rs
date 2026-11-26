//! v2 允许动作的唯一 owner：本人读模型与 Admin 读模型都由这里产生动作清单。
//!
//! 不变量 6：只宣告服务端此刻真能执行的动作，命令守卫的放行范围只会比这里更宽；新增命令或
//! 放宽守卫时必须先改这里并补测试。第 5 期没有 v2 改价命令，所以代管态不宣告 `Reprice`；
//! 重列只在读侧按重列写端同一判据给出 `relist_allowed` 时宣告，最终仍由写端锁内裁决。

use uuid::Uuid;

use super::custody::PositionState;
use crate::consignment::{HeldOffMarketReason, ListingState};

/// 本人可执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SellerAction {
    /// 修改标价；仅未代管态，走 v1 改价命令。
    Reprice,
    /// 撤回或取消；未代管件直接关闭，代管件登记撤回意愿。
    Withdraw,
    /// 从在库下架恢复销售；只在 `ActionFacts::relist_allowed` 为真时产生。
    Relist,
    /// 签发或换发寄件预约。
    IssueReservation,
}

impl SellerAction {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reprice => "reprice",
            Self::Withdraw => "withdraw",
            Self::Relist => "relist",
            Self::IssueReservation => "issue_reservation",
        }
    }
}

/// Admin 可执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminAction {
    /// 下架：未代管件关闭，代管件设运营止售。
    Delist,
    /// 解除当前有效的运营止售。
    ReleaseHold,
}

impl AdminAction {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delist => "delist",
            Self::ReleaseHold => "release_hold",
        }
    }
}

/// 产生动作所需的同一读快照事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionFacts {
    /// 有效状态（已计入强制终结）。
    pub effective_state: ListingState,
    /// 在库下架的原因；其他状态为 `None`。
    pub held_reason: Option<HeldOffMarketReason>,
    /// 存在未解除的 `hold_set`。
    pub active_hold: bool,
    /// 存在撤回意愿控制行。
    pub withdrawal_intent_registered: bool,
    /// 挂单的代管 position 状态；未代管为 `None`。
    pub position: Option<PositionState>,
    /// 重列前置条件此刻全部满足；由存储读侧按重列写端同一判据给出，读不出时为否。
    pub relist_allowed: bool,
    /// 关联的损失案件；第 7 期之前恒为 `None`。
    pub loss_case_id: Option<Uuid>,
    /// 卖家经站内账户登记；匿名非寄售挂单为否。
    pub seller_verified: bool,
}

/// 代管态的命令都要求 position 仍在未终结的代管阶段；终态 position 不再接受控制命令。
fn custody_actionable(facts: &ActionFacts) -> bool {
    matches!(
        facts.position,
        Some(PositionState::AwaitingTake | PositionState::InCustody | PositionState::Reconcile)
    )
}

/// 本人此刻可执行的动作。
pub fn seller_actions(facts: &ActionFacts) -> Vec<SellerAction> {
    let mut actions = match facts.effective_state {
        ListingState::Resolving | ListingState::Displayed => {
            vec![SellerAction::Reprice, SellerAction::Withdraw]
        }
        ListingState::AwaitingDeposit => {
            vec![SellerAction::Reprice, SellerAction::Withdraw, SellerAction::IssueReservation]
        }
        // 已登记意愿后再次撤回只是原样重放，不再宣告。
        ListingState::DepositAccepted | ListingState::Available | ListingState::HeldOffMarket
            if custody_actionable(facts) && !facts.withdrawal_intent_registered =>
        {
            vec![SellerAction::Withdraw]
        }
        _ => Vec::new(),
    };
    // 重列只针对在库下架件；资格本身由读侧按写端判据给出，这里不再重建条件。决定 24：运营止售原因的件
    // 不宣告重列（DTO 校验同口径）；有效数据里该原因必有有效止售，写端同样拒绝，这里只防畸形行。
    if facts.relist_allowed
        && facts.effective_state == ListingState::HeldOffMarket
        && facts.held_reason != Some(HeldOffMarketReason::OperationsHold)
    {
        actions.push(SellerAction::Relist);
    }
    // 匿名非寄售挂单不可改价，同名账户只能撤回（维护者决定 2）。
    if !facts.seller_verified {
        actions.retain(|action| *action == SellerAction::Withdraw);
    }
    actions
}

/// Admin 此刻可执行的动作。
pub fn admin_actions(facts: &ActionFacts) -> Vec<AdminAction> {
    match facts.effective_state {
        ListingState::Resolving | ListingState::Displayed | ListingState::AwaitingDeposit => {
            vec![AdminAction::Delist]
        }
        ListingState::DepositAccepted | ListingState::Available | ListingState::HeldOffMarket
            if custody_actionable(facts) =>
        {
            // 同一 position 已有有效止售时再设止售会被拒绝，只能解除；解除要求在库下架态。
            match (facts.active_hold, facts.effective_state) {
                (false, _) => vec![AdminAction::Delist],
                (true, ListingState::HeldOffMarket) => vec![AdminAction::ReleaseHold],
                (true, _) => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ActionFacts, AdminAction, SellerAction, admin_actions, seller_actions};
    use crate::consignment::ListingState;
    use crate::consignment::deposit::PositionState;

    fn facts(state: ListingState, position: Option<PositionState>) -> ActionFacts {
        ActionFacts {
            effective_state: state,
            held_reason: None,
            active_hold: false,
            withdrawal_intent_registered: false,
            position,
            relist_allowed: false,
            loss_case_id: None,
            seller_verified: true,
        }
    }

    #[test]
    fn anonymous_listings_offer_withdraw_but_never_reprice() {
        for state in [ListingState::Resolving, ListingState::Displayed] {
            let anonymous = ActionFacts { seller_verified: false, ..facts(state, None) };
            assert_eq!(seller_actions(&anonymous), vec![SellerAction::Withdraw], "{state:?}");
            assert_eq!(admin_actions(&anonymous), vec![AdminAction::Delist], "{state:?}");
        }
        let closed = ActionFacts { seller_verified: false, ..facts(ListingState::Closed, None) };
        assert!(seller_actions(&closed).is_empty());
    }

    #[test]
    fn uncustodied_states_offer_reprice_and_withdraw_only_where_the_commands_run() {
        for (state, expected) in [
            (ListingState::Resolving, vec![SellerAction::Reprice, SellerAction::Withdraw]),
            (ListingState::Displayed, vec![SellerAction::Reprice, SellerAction::Withdraw]),
            (
                ListingState::AwaitingDeposit,
                vec![SellerAction::Reprice, SellerAction::Withdraw, SellerAction::IssueReservation],
            ),
        ] {
            assert_eq!(seller_actions(&facts(state, None)), expected, "{state:?}");
            assert_eq!(admin_actions(&facts(state, None)), vec![AdminAction::Delist], "{state:?}");
        }
    }

    #[test]
    fn custody_states_never_offer_reprice_or_relist() {
        for state in
            [ListingState::DepositAccepted, ListingState::Available, ListingState::HeldOffMarket]
        {
            let actions = seller_actions(&facts(state, Some(PositionState::InCustody)));
            assert_eq!(actions, vec![SellerAction::Withdraw], "{state:?}");
        }
    }

    #[test]
    fn registered_intent_and_terminal_positions_close_the_withdraw_offer() {
        let mut intent = facts(ListingState::Available, Some(PositionState::InCustody));
        intent.withdrawal_intent_registered = true;
        assert!(seller_actions(&intent).is_empty());
        for position in [None, Some(PositionState::HandedOver), Some(PositionState::Lost)] {
            let custody = facts(ListingState::Available, position);
            assert!(seller_actions(&custody).is_empty(), "{position:?}");
            assert!(admin_actions(&custody).is_empty(), "{position:?}");
        }
    }

    #[test]
    fn active_hold_swaps_delist_for_release_only_while_held_off_market() {
        let mut held = facts(ListingState::HeldOffMarket, Some(PositionState::InCustody));
        assert_eq!(admin_actions(&held), vec![AdminAction::Delist]);
        held.active_hold = true;
        assert_eq!(admin_actions(&held), vec![AdminAction::ReleaseHold]);
        let mut stray = facts(ListingState::Available, Some(PositionState::InCustody));
        stray.active_hold = true;
        assert!(admin_actions(&stray).is_empty());
    }

    #[test]
    fn pending_and_terminal_states_offer_nothing() {
        for state in [
            ListingState::SalePending,
            ListingState::WithdrawalPending,
            ListingState::Rejected,
            ListingState::Expired,
            ListingState::Closed,
        ] {
            let custody = facts(state, Some(PositionState::InCustody));
            assert!(seller_actions(&custody).is_empty(), "{state:?}");
            assert!(admin_actions(&custody).is_empty(), "{state:?}");
        }
    }
}
