//! 结算领域模型的模块装配。
//!
//! 策略、金额、结算事实、后续义务和最终计划分别由各自 owner 维护；本文件只提供稳定
//! 的领域导出，不承载计算或状态规则。

mod amount;
mod facts;
mod obligations;
mod plan;
mod policy;

pub use amount::SettlementAmount;
pub use facts::{SettlementFact, SettlementLine, SettlementWorld};
pub use obligations::{SettlementObligation, SettlementObligationKind, SettlementObligationState};
pub use plan::{SettlementPlan, floor_ppm, proportional_floor};
pub use policy::{
    AdjustmentAnchor, RecoveryMode, RecoveryPolicy, SETTLEMENT_PPM, SettlementMode,
    SettlementPolicy,
};
