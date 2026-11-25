//! 权利种类、可执行动作与权利视图。
//!
//! 服务端只给出能立即执行的动作子集；损失权利只读，没有任何动作。

use uuid::Uuid;

use crate::SettlementWorld;

/// 权利种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RightKind {
    /// 续办订单。
    ContinueOrder,
    /// 领取装备。
    ClaimEquipment,
    /// 领取款项。
    ClaimPayment,
    /// 取回代管件。
    WithdrawHeld,
    /// 损失案件，只读。
    LossCase,
}

impl RightKind {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContinueOrder => "continue_order",
            Self::ClaimEquipment => "claim_equipment",
            Self::ClaimPayment => "claim_payment",
            Self::WithdrawHeld => "withdraw_held",
            Self::LossCase => "loss_case",
        }
    }

    /// 该种类对应的唯一动作；损失案件没有。
    pub const fn action(self) -> Option<RightAction> {
        match self {
            Self::ContinueOrder => Some(RightAction::ContinueOrder),
            Self::ClaimEquipment => Some(RightAction::ClaimEquipment),
            Self::ClaimPayment => Some(RightAction::ClaimPayment),
            Self::WithdrawHeld => Some(RightAction::WithdrawHeld),
            Self::LossCase => None,
        }
    }
}

/// 受限会话可请求的动作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RightAction {
    /// 续办订单。
    ContinueOrder,
    /// 领取装备。
    ClaimEquipment,
    /// 领取款项。
    ClaimPayment,
    /// 取回代管件。
    WithdrawHeld,
}

impl RightAction {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ContinueOrder => "continue_order",
            Self::ClaimEquipment => "claim_equipment",
            Self::ClaimPayment => "claim_payment",
            Self::WithdrawHeld => "withdraw_held",
        }
    }

    /// 严格解析 wire 码；未知值返回 `None`，调用方失败关闭。
    pub fn parse(text: &str) -> Option<Self> {
        [Self::ContinueOrder, Self::ClaimEquipment, Self::ClaimPayment, Self::WithdrawHeld]
            .into_iter()
            .find(|action| action.as_str() == text)
    }
}

/// 权利指向的合同或代管件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightTarget {
    /// 寄售合同。
    Contract(Uuid),
    /// 代管 position。
    Position(Uuid),
}

/// 锁内读到的、决定动作可否执行的事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RightActionFacts {
    /// 权利仍有效（未封闭、未被取代）。
    pub active: bool,
    /// 订单当前可续办。
    pub continue_order: bool,
    /// 装备当前可领取。
    pub claim_equipment: bool,
    /// 款项当前可领取。
    pub claim_payment: bool,
    /// 代管件当前可取回。
    pub withdraw_held: bool,
}

/// 只产生服务端当前可执行的动作子集；权利失效或种类无动作时为空。
pub fn allowed_actions(kind: RightKind, facts: &RightActionFacts) -> Vec<RightAction> {
    if !facts.active {
        return Vec::new();
    }
    let executable = match kind {
        RightKind::ContinueOrder => facts.continue_order,
        RightKind::ClaimEquipment => facts.claim_equipment,
        RightKind::ClaimPayment => facts.claim_payment,
        RightKind::WithdrawHeld => facts.withdraw_held,
        RightKind::LossCase => false,
    };
    match kind.action() {
        Some(action) if executable => vec![action],
        _ => Vec::new(),
    }
}

/// 受限会话读到的权利视图；不含任何定位符、码或令牌。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RightView {
    /// 权利 UUID。
    pub right_id: Uuid,
    /// 权利版本。
    pub right_version: u64,
    /// 种类。
    pub kind: RightKind,
    /// 世界。
    pub world: SettlementWorld,
    /// 指向的合同或 position。
    pub contract_or_position: RightTarget,
    /// 当前可执行动作。
    pub allowed_actions: Vec<RightAction>,
}

#[cfg(test)]
mod tests {
    use super::{RightAction, RightActionFacts, RightKind, allowed_actions};

    fn all_true() -> RightActionFacts {
        RightActionFacts {
            active: true,
            continue_order: true,
            claim_equipment: true,
            claim_payment: true,
            withdraw_held: true,
        }
    }

    #[test]
    fn each_kind_only_grants_its_own_executable_action() {
        assert_eq!(
            allowed_actions(RightKind::ClaimEquipment, &all_true()),
            vec![RightAction::ClaimEquipment]
        );
        let facts = RightActionFacts { claim_payment: false, ..all_true() };
        assert!(allowed_actions(RightKind::ClaimPayment, &facts).is_empty());
    }

    #[test]
    fn loss_case_and_inactive_rights_have_no_actions() {
        assert!(allowed_actions(RightKind::LossCase, &all_true()).is_empty());
        let facts = RightActionFacts { active: false, ..all_true() };
        assert!(allowed_actions(RightKind::WithdrawHeld, &facts).is_empty());
    }

    #[test]
    fn action_codes_parse_strictly() {
        assert_eq!(RightAction::parse("claim_payment"), Some(RightAction::ClaimPayment));
        assert_eq!(RightAction::parse("relist"), None);
        assert_eq!(RightAction::parse("Claim_Payment"), None);
    }
}
