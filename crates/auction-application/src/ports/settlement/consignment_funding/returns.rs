//! 退款转换命令与回执。

use auction_domain::consignment::funding::{FeeReversal, RefundConversion};
use auction_domain::consignment::{ListingPrice, ListingQuote};
use uuid::Uuid;

use crate::ports::settlement::FundingMutation;

/// 键 = Sale + 原退款义务。
#[derive(Debug, Clone)]
pub struct ConvertRefund {
    pub mutation: FundingMutation,
    pub contract_id: Uuid,
    pub refund_obligation_id: Uuid,
    pub price: ListingPrice,
    pub quote: ListingQuote,
}

/// 转换结果；F = 0 时 `fee_reversal` 为空。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundReceipt {
    pub conversion: RefundConversion,
    pub fee_reversal: Option<FeeReversal>,
    pub replayed: bool,
}
