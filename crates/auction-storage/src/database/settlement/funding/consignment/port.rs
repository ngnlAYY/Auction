//! `ConsignmentFundingStore` 的 PostgreSQL 实现：四个写口只委托本目录的存储入口，不复制任何资金逻辑。
//!
//! 每个入口自带顶层 Serializable 事务、锁序、命令幂等与有界重试（不变量 1、2、3、6），见各自模块。

use auction_application::settlement::{
    AttributionReceipt, CashReceipt, CertifyCod, CertifyReceipt, ConsignmentFundingStore,
    ConvertRefund, FundingFuture, RefundReceipt, RegisterOpeningAttribution, SettleCash,
};

use super::cash_legs::settle_cash;
use super::certify_cod::certify_cod;
use super::opening_slice::register_opening_attribution;
use super::refund_conversion::convert_refund;
use crate::DatabaseStore;

impl ConsignmentFundingStore for DatabaseStore {
    fn register_opening_attribution(
        &self,
        command: RegisterOpeningAttribution,
    ) -> FundingFuture<'_, AttributionReceipt> {
        Box::pin(register_opening_attribution(self, command))
    }

    fn certify_cod(&self, command: CertifyCod) -> FundingFuture<'_, CertifyReceipt> {
        Box::pin(certify_cod(self, command))
    }

    fn settle_cash(&self, command: SettleCash) -> FundingFuture<'_, CashReceipt> {
        Box::pin(settle_cash(self, command))
    }

    fn convert_refund(&self, command: ConvertRefund) -> FundingFuture<'_, RefundReceipt> {
        Box::pin(convert_refund(self, command))
    }
}
