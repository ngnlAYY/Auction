//! opening 份额归属登记命令。

use auction_domain::consignment::funding::OpeningSlice;
use uuid::Uuid;

use super::certify::ReceiptKey;
use crate::ports::settlement::FundingMutation;

/// 封存同一经济实收可能已含的 opening 份额；键 = 实收业务键 + 份额区间。
#[derive(Debug, Clone)]
pub struct RegisterOpeningAttribution {
    pub mutation: FundingMutation,
    pub receipt: ReceiptKey,
    pub slices: Vec<OpeningSlice>,
}

/// 登记结果；`included` 为各段金额之和 A。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributionReceipt {
    pub attribution_id: Uuid,
    pub included: i64,
    pub replayed: bool,
}
