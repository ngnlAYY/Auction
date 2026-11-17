//! 结算退件：来源认证、正文解析、证据消费判定、案件 Admin 端口与 Worker 端口。
//!
//! 收件人拒收（或过期）的结算邮件由系统发件人 `MoogleMail` 以 `[RTS] ` + 原 canonical 主题
//! 退回 Bot 收件箱。证据按入站邮件只追加；案件是可变投影，处理动作与事实另行只追加。

mod body;
mod cases;
mod evidence;
mod source;
mod work;

pub use body::{ReturnedSettlementBody, parse_returned_settlement_body, returned_mail_body_digest};
pub use cases::{
    MAX_SETTLEMENT_RETURN_OFFSET, MAX_SETTLEMENT_RETURN_PAGE_SIZE, SettlementReturnAdminPort,
    SettlementReturnAssetLocation, SettlementReturnCaseSummary, SettlementReturnCommand,
    SettlementReturnCommandResult, SettlementReturnCustody, SettlementReturnDetail,
    SettlementReturnDispatch, SettlementReturnError, SettlementReturnEvent,
    SettlementReturnListRequest, SettlementReturnPage, SettlementReturnTake,
};
pub use evidence::{
    RecoveryReturnEvidence, SettlementReturnAssets, SettlementReturnConsumption,
    SettlementReturnDecision, SettlementReturnDelivery, SettlementReturnDispatchLookup,
    SettlementReturnDispatchRequest, SettlementReturnFacts, SettlementReturnMatch,
    SettlementReturnObligation, SettlementReturnReason, recovery_return_evidence,
    settlement_return_assets, settlement_return_dispatch_request, settlement_return_original_body,
};
pub use source::{
    SYSTEM_RETURN_SENDER, system_return_matches_obligation,
    system_returned_settlement_obligation_id,
};
pub use work::{
    SettlementReturnMailObservation, SettlementReturnMailReader, SettlementReturnProgress,
    SettlementReturnRecheckClaim, SettlementReturnRecheckOutcome, SettlementReturnWorkStore,
};
