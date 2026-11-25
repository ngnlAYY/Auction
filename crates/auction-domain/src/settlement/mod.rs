//! 结算领域模型的模块装配；规则由各子模块持有，本文件只提供稳定导出，不承载计算或状态规则。

mod amount;
pub mod consignment_funding;
mod equipment_loss;
pub use equipment_loss::equipment_loss_funding;
mod equipment_loss_admission;
pub use equipment_loss_admission::{EquipmentLossAdmission, EquipmentLossBlocker};
mod facts;
mod funding;
pub use funding::{FundingAccountKey, FundingSplit, funding_reconciliation_capacity};
mod internal;
pub use internal::InternalSettlementAction;
mod obligations;
mod origin;
mod plan;
mod policy;
mod recovery_advance;
mod returns;
mod reversal;

pub use amount::SettlementAmount;
pub use facts::{
    SettlementFact, SettlementLine, SettlementWorld, validate_settlement_price_breakdown_for_mode,
};
pub(crate) use obligations::{ObligationShape, validate_obligation_shape};
pub use obligations::{SettlementObligation, SettlementObligationKind, SettlementObligationState};
pub use origin::{ConsignmentContractKind, SettlementOrigin};
pub use plan::{SettlementPlan, floor_ppm, proportional_floor};
pub(crate) use policy::RECOVERY_V2_PAYMENT_TIMEOUT;
pub use policy::{
    AdjustmentAnchor, DEFAULT_COD_RETURN_LIMIT, RecoveryMode, RecoveryPolicy, SETTLEMENT_PPM,
    SettlementMode, SettlementPolicy,
};
pub use recovery_advance::{
    RecoveryAdvance, RecoveryAdvanceError, RecoveryAdvanceFacts, RecoveryStage,
    next_recovery_advance,
};
pub use returns::{
    DISPATCH_REFERENCE_LINE_PREFIX, FIRST_DISPATCH_NO, SETTLEMENT_RETURN_RESEND_LIMIT,
    SettlementDispatchReference, SettlementReturnAction, SettlementReturnAdmission,
    SettlementReturnBinding, SettlementReturnBlocker, SettlementReturnCaseStatus,
    SettlementReturnFunding, SettlementReturnObligationPosition, SettlementReturnPlan,
    SettlementReturnTakeUnknownReason, cash_return, resend_activate_command_id, resend_delivery_id,
    resend_prepare_command_id, resend_supported,
};
pub use reversal::{COMPENSATION_REVERSAL_RULE_VERSION, CompensationReversal};
