//! 购买判定：按冻结的拒绝优先序校验锁内事实，全部满足才授予成交金额。

use super::facts::{PurchaseFacts, PurchaseFunding, PurchaseInput, PurchaseParty};
use super::rejection::PurchaseRejection;
use crate::SettlementWorld;
use crate::consignment::deposit::PositionState;
use crate::consignment::{
    FeeBps, ListingMode, ListingPrice, ListingQuote, ListingState, ListingVersion, PRICING_VERSION,
    SaleFundingFreeze, SeasonState, quote_v1,
};

/// 购买授予的成交事实；金额由 `quote_v1` 重算，F + S = P。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurchaseGrant {
    pub price: ListingPrice,
    pub quote: ListingQuote,
    pub fee_bps: FeeBps,
    pub pricing_version: u16,
    /// 成交后挂单进入 `sale_pending` 所用的新版本。
    pub listing_version_after: ListingVersion,
    /// 被本次成交消耗的处分代。
    pub generation: u64,
    /// 与判定同源的资金预检冻结值，成交时原样写入 Sale。
    pub funding: SaleFundingFreeze,
}

/// 判定购买。拒绝优先序（冻结，改序须新合同）：
/// `ListingNotActive` → `OperationsHold` → `StaleVersion` → `PriceChanged` →
/// `EquipmentCommitted` → `CustodyUnproven` → `SeasonClosed` → `BotParty` →
/// `SelfPurchase` → `ParticipantIneligible`（含待执行处罚）→ `FundingUnavailable`。
///
/// 保护不变量 2、6：同一份锁内快照得出唯一结论，任何一项不满足或无法判定都失败关闭，
/// 不静默修正；成交时金额只由冻结的价格与费率重算。
pub fn decide_purchase(
    input: &PurchaseInput,
    facts: &PurchaseFacts,
) -> Result<PurchaseGrant, PurchaseRejection> {
    let listing = &facts.listing;
    // 服务关闭、世代过期、模式或计价版本不符都等价于挂单不再接受购买。
    let listing_open = facts.service_enabled
        && facts.listing_epoch_current
        && input.listing_id == listing.listing_id
        && listing.mode == ListingMode::Consigned
        && listing.state == ListingState::Available
        && listing.pricing_version == PRICING_VERSION;
    if !listing_open {
        return Err(PurchaseRejection::ListingNotActive);
    }
    if facts.active_hold {
        return Err(PurchaseRejection::OperationsHold);
    }
    if input.expected_version != listing.version {
        return Err(PurchaseRejection::StaleVersion);
    }
    if input.expected_price != listing.price {
        return Err(PurchaseRejection::PriceChanged);
    }
    let position = facts.position.as_ref();
    let committed = facts.opposite_unknown_write
        || facts.unclosed_delivery
        || position.is_some_and(|position| {
            position.generation_consumed
                || matches!(position.state, PositionState::Committed | PositionState::InTransit)
        });
    if committed {
        return Err(PurchaseRejection::EquipmentCommitted);
    }
    // 严格取得与连续持有：必须是已证取得、仍处于代管、带处分代的 position。
    let generation = position
        .filter(|position| position.state == PositionState::InCustody && position.take_proven)
        .and_then(|position| position.generation)
        .filter(|generation| *generation >= 1)
        .ok_or(PurchaseRejection::CustodyUnproven)?;
    if !season_allows_purchase(listing.world, facts.season) {
        return Err(PurchaseRejection::SeasonClosed);
    }
    if facts.buyer.is_authenticated_bot || facts.seller.is_authenticated_bot {
        return Err(PurchaseRejection::BotParty);
    }
    // 决定 25：同一本地账号不得自买，不看昵称或 Bot 之外的任何别名。
    if facts.buyer.user_id == facts.seller.user_id {
        return Err(PurchaseRejection::SelfPurchase);
    }
    if !party_may_trade(&facts.buyer)
        || !party_may_trade(&facts.seller)
        || facts.buyer_sanction_pending
    {
        return Err(PurchaseRejection::ParticipantIneligible);
    }
    let PurchaseFunding::Ready(funding) = &facts.funding else {
        return Err(PurchaseRejection::FundingUnavailable);
    };
    let listing_version_after =
        listing.version.next().ok_or(PurchaseRejection::ListingNotActive)?;
    Ok(PurchaseGrant {
        price: listing.price,
        quote: quote_v1(listing.price, listing.fee_bps),
        fee_bps: listing.fee_bps,
        pricing_version: PRICING_VERSION,
        listing_version_after,
        generation,
        funding: funding.clone(),
    })
}

fn party_may_trade(party: &PurchaseParty) -> bool {
    party.active && party.eligible
}

/// 只有 main（无季界）与 isekai 的开放季允许新成交；预停、关闭、未知与世界和季态不符一律拒绝。
fn season_allows_purchase(world: SettlementWorld, season: SeasonState) -> bool {
    matches!(
        (world, season),
        (SettlementWorld::Main, SeasonState::NotSeasonal)
            | (SettlementWorld::Isekai, SeasonState::Open)
    )
}
