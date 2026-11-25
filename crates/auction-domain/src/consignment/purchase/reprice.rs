//! v2 改价（R5-C11）的领域判定：只允许 `available` 且尚无 Sale 的挂单。

use uuid::Uuid;

use super::facts::{PurchaseListing, PurchasePosition};
use crate::UserId;
use crate::consignment::deposit::PositionState;
use crate::consignment::{ListingPrice, ListingQuote, ListingState, ListingVersion, quote_v1};

/// 改价命令携带的预期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepriceInput {
    /// 发起改价的卖家；必须等于锁内事实里的挂单卖家。
    pub seller_user_id: UserId,
    pub listing_id: Uuid,
    pub expected_version: ListingVersion,
    pub price: ListingPrice,
}

/// 改价的锁内事实，与购买共用挂单与 position 快照类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepriceFacts {
    /// 挂单的卖家。
    pub seller_user_id: UserId,
    pub listing: PurchaseListing,
    pub position: Option<PurchasePosition>,
    /// 该挂单已存在 Sale 合同。
    pub sale_exists: bool,
}

/// 改价被确定拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepriceRejection {
    /// 发起人不是挂单卖家或挂单不存在；不泄露挂单存在（HTTP 404）。
    ListingNotFound,
    ListingNotActive,
    StaleVersion,
    EquipmentCommitted,
}

impl RepriceRejection {
    /// 账本与 API 使用的稳定业务码，与购买拒绝同名同义。
    pub const fn code(self) -> &'static str {
        match self {
            Self::ListingNotFound => "listing_not_found",
            Self::ListingNotActive => "listing_not_active",
            Self::StaleVersion => "stale_version",
            Self::EquipmentCommitted => "equipment_committed",
        }
    }
}

/// 改价结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepriceDecision {
    /// 价格变化，挂单版本推进一个。
    Changed { price: ListingPrice, quote: ListingQuote, listing_version_after: ListingVersion },
    /// 同价，回执为 `unchanged`，不增版本。
    Unchanged,
}

/// 判定 v2 改价。
///
/// 保护不变量 2、6：只有原卖家可改价，非卖家得 `ListingNotFound`、无任何变更；已成交（有 Sale、`sale_pending`、position 已承诺或在途）的挂单价格冻结在合同里，
/// 改价一律拒绝；只有 `available` 可改，不静默修正。
pub fn decide_reprice_available(
    input: &RepriceInput,
    facts: &RepriceFacts,
) -> Result<RepriceDecision, RepriceRejection> {
    let listing = &facts.listing;
    if input.seller_user_id != facts.seller_user_id {
        return Err(RepriceRejection::ListingNotFound);
    }
    if input.listing_id != listing.listing_id || !listing.state.is_active() {
        return Err(RepriceRejection::ListingNotActive);
    }
    let committed = facts.sale_exists
        || listing.state == ListingState::SalePending
        || facts.position.is_some_and(|position| {
            position.generation_consumed
                || matches!(position.state, PositionState::Committed | PositionState::InTransit)
        });
    if committed {
        return Err(RepriceRejection::EquipmentCommitted);
    }
    if listing.state != ListingState::Available {
        return Err(RepriceRejection::ListingNotActive);
    }
    if input.expected_version != listing.version {
        return Err(RepriceRejection::StaleVersion);
    }
    if input.price == listing.price {
        return Ok(RepriceDecision::Unchanged);
    }
    let listing_version_after = listing.version.next().ok_or(RepriceRejection::ListingNotActive)?;
    Ok(RepriceDecision::Changed {
        price: input.price,
        quote: quote_v1(input.price, listing.fee_bps),
        listing_version_after,
    })
}
