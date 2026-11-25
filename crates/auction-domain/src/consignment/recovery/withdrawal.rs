//! 直接撤回的领域规则：当前代未售则直接绑定，Sale 已封闭则追加 `withdrawal_only` 代。
//!
//! 输入是锁内读好的事实，输出是决定；不读库、不读时钟。

use thiserror::Error;

use crate::consignment::deposit::PositionState;

/// 当前处分代相对买方交割权的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentGeneration {
    /// 当前代未授合同。
    Unsold,
    /// Sale 已占代 `generation`，其交割、退款与卖款处置全部封闭且原件已收回。
    SaleSealed {
        /// 当前代序号 g；后继代为 g+1。
        generation: u64,
        /// 上代封闭证明。
        predecessor_proof: uuid::Uuid,
    },
    /// 旧买方交割未封闭或仍有资金责任。
    SaleOpen,
}

/// 直接撤回的锁内事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectWithdrawalFacts {
    /// 请求方仍是取回权人。
    pub seller_is_rights_holder: bool,
    /// position 状态；只有 `InCustody` 才能取物。
    pub position_state: PositionState,
    /// 当前代状态。
    pub generation: CurrentGeneration,
    /// 存在相反的未知 Send / Take / Return。
    pub opposite_unknown_write: bool,
    /// 当季仍能安全寄还。
    pub safe_to_return_in_season: bool,
    /// 独立运营止售；决定 24：不阻拦取物，仅为显式输入以便断言。
    pub active_hold: bool,
}

/// 直接撤回被拒绝的原因；调用方映射为确定业务码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DirectWithdrawalRejection {
    /// 请求方不是取回权人。
    #[error("请求方不是取回权人")]
    NotRightsHolder,
    /// 当前代仍有买方交割权或资金责任（`equipment_committed`）。
    #[error("装备已被买方权利占用")]
    EquipmentCommitted,
    /// 原件未证明在代管中，或存在相反未知外写（`custody_unproven`）。
    #[error("代管状态未证明")]
    CustodyUnproven,
    /// 当季已无法安全寄还（`season_closed`）。
    #[error("当季已无法安全寄还")]
    SeasonClosed,
}

/// 直接撤回的决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectWithdrawalDecision {
    /// 当前代未售：把 Withdrawal 直接绑当前代。
    BindCurrentGeneration,
    /// Sale 已封闭：追加 g+1、`purpose=withdrawal_only`、predecessor=g。
    AppendWithdrawalOnlyGeneration {
        /// 前代序号 g。
        predecessor: u64,
        /// 上代封闭证明。
        predecessor_proof: uuid::Uuid,
    },
    /// 拒绝。
    Reject(DirectWithdrawalRejection),
}

/// 不变量 2、6：任何一项事实不成立即失败关闭；未知交割保持占用待核，不猜测放行。
pub fn decide_direct_withdrawal(facts: &DirectWithdrawalFacts) -> DirectWithdrawalDecision {
    use DirectWithdrawalDecision::{AppendWithdrawalOnlyGeneration, BindCurrentGeneration, Reject};
    use DirectWithdrawalRejection as Rejection;
    if !facts.seller_is_rights_holder {
        return Reject(Rejection::NotRightsHolder);
    }
    if facts.position_state != PositionState::InCustody {
        return Reject(Rejection::CustodyUnproven);
    }
    if matches!(facts.generation, CurrentGeneration::SaleOpen) {
        return Reject(Rejection::EquipmentCommitted);
    }
    if facts.opposite_unknown_write {
        return Reject(Rejection::CustodyUnproven);
    }
    if !facts.safe_to_return_in_season {
        return Reject(Rejection::SeasonClosed);
    }
    match facts.generation {
        CurrentGeneration::Unsold => BindCurrentGeneration,
        CurrentGeneration::SaleSealed { generation, predecessor_proof } => {
            AppendWithdrawalOnlyGeneration { predecessor: generation, predecessor_proof }
        }
        CurrentGeneration::SaleOpen => Reject(Rejection::EquipmentCommitted),
    }
}
