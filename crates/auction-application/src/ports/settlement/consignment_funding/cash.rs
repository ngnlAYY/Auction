//! 现金义务状态迁移命令。

use uuid::Uuid;

use crate::ports::settlement::FundingMutation;

/// 现金腿：载体本金 1、卖款 S、退款 P。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CashLeg {
    CarrierPrincipal,
    Seller,
    Refund,
}

/// 状态迁移阶段；只迁移同用途状态，不增加资产。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CashPhase {
    Reserve,
    InFlight,
    Unknown,
    Spent,
    Claimed,
    Retained,
}

/// 键 = 现金义务 + delivery + 阶段。
#[derive(Debug, Clone)]
pub struct SettleCash {
    pub mutation: FundingMutation,
    pub contract_id: Uuid,
    pub obligation_id: Uuid,
    pub delivery_id: Uuid,
    pub leg: CashLeg,
    pub phase: CashPhase,
    pub amount: i64,
}

/// 回执只带是否重放。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CashReceipt {
    pub replayed: bool,
}
