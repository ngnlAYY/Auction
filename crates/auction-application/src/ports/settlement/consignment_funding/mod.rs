//! 寄售资金的窄端口：只读预检、收款认证、现金支出、退款转换与 UID 级 opening 门判定。
//!
//! 命令只带业务键与摘要；金额与分账规则由 `auction_domain::consignment::funding` 给出，
//! 存储实现（第 6 期 6B2 / 6B3）在同一事务内落实，应用层不直接取资金锁。

mod cash;
mod certify;
mod opening;
mod preflight;
mod returns;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use cash::{CashLeg, CashPhase, CashReceipt, SettleCash};
pub use certify::{
    CertifyCod, CertifyReceipt, CodLeg, DispatchFreeze, FreshCashObservation, ReceiptKey,
};
pub use opening::{AttributionReceipt, RegisterOpeningAttribution};
pub use preflight::{ConsignmentFundingPreflight, FundingPreflightFact, FundingUnavailable};
pub use returns::{ConvertRefund, RefundReceipt};

use crate::ports::settlement::FundingFuture;

/// 寄售资金写端；每个方法对应一个事务，同命令 ID 同摘要返回原结果，同 ID 异摘要冲突。
pub trait ConsignmentFundingStore: Send + Sync {
    /// 封存并锁定 opening 份额的逐笔依据；不发行、不转移。拒绝存在阻断的 UID。
    fn register_opening_attribution(
        &self,
        command: RegisterOpeningAttribution,
    ) -> FundingFuture<'_, AttributionReceipt>;

    /// 寄售付款义务转 satisfied 的唯一入口。
    fn certify_cod(&self, command: CertifyCod) -> FundingFuture<'_, CertifyReceipt>;

    /// 现金义务的 reserve / in_flight / unknown / spent / claimed / retained 状态迁移。
    fn settle_cash(&self, command: SettleCash) -> FundingFuture<'_, CashReceipt>;

    /// 已证退款原因下把卖款与手续费转为买家退款，并冲回本笔手续费。
    fn convert_refund(&self, command: ConvertRefund) -> FundingFuture<'_, RefundReceipt>;
}
