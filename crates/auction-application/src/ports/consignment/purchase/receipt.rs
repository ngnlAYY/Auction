//! 购买与 v2 改价的回执；确定业务拒绝留安全回执，随账本重放。

use auction_domain::consignment::purchase::{PurchaseRejection, RepriceRejection};
use auction_domain::consignment::{ListingPrice, ListingVersion, OrderState};
use auction_domain::{CommandId, ConsignmentContractId, SettlementWorld};
use uuid::Uuid;

use crate::ports::consignment::ListingPriceDisposition;

/// `201 purchased` 回执；`replayed` 表示命中同命令 ID 的原结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseReceipt {
    pub command_id: CommandId,
    pub listing_id: Uuid,
    /// 成交后挂单（`sale_pending`）的版本。
    pub listing_version: ListingVersion,
    pub contract_id: ConsignmentContractId,
    pub contract_version: u64,
    pub equipment_id: Uuid,
    pub world: SettlementWorld,
    pub price: ListingPrice,
    /// 购买后恒为 `awaiting_payment`。
    pub order_state: OrderState,
    pub replayed: bool,
}

/// 购买结果：成交，或带安全码的确定拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurchaseOutcome {
    Purchased(PurchaseReceipt),
    Rejected(PurchaseRejection),
}

/// v2 改价回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepriceV2Receipt {
    pub command_id: CommandId,
    pub listing_id: Uuid,
    pub listing_version: ListingVersion,
    pub price: ListingPrice,
    pub disposition: ListingPriceDisposition,
    pub replayed: bool,
}

/// v2 改价结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepriceV2Outcome {
    Accepted(RepriceV2Receipt),
    Rejected(RepriceRejection),
}
