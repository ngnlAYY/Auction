//! 结算领域模型的模块装配。
//! 策略、金额、结算事实、后续义务、最终计划和成交撤销冲正分别由各自 owner 维护；
//! 本文件只提供稳定的领域导出，不承载计算或状态规则。

mod amount;
mod facts;
mod funding;
pub use funding::{FundingAccountKey, FundingSplit, funding_reconciliation_capacity};
mod internal;
pub use internal::InternalSettlementAction;
mod obligations;
mod plan;
mod policy;
mod reversal;

pub use amount::SettlementAmount;
pub use facts::{
    SettlementFact, SettlementLine, SettlementWorld, validate_settlement_price_breakdown,
    validate_settlement_price_breakdown_for_mode,
};
pub use obligations::{SettlementObligation, SettlementObligationKind, SettlementObligationState};
pub use plan::{SettlementPlan, floor_ppm, proportional_floor};
pub use policy::{
    AdjustmentAnchor, DEFAULT_COD_RETURN_LIMIT, MAX_COD_RETURN_LIMIT, RECOVERY_V2_PAYMENT_TIMEOUT,
    RecoveryMode, RecoveryPolicy, SETTLEMENT_PPM, SettlementMode, SettlementPolicy,
};
pub use reversal::{COMPENSATION_REVERSAL_RULE_VERSION, CompensationReversal};
