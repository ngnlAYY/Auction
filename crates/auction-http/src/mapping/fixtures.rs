//! v2 挂单读模型的内存夹具，供 Public 与 Admin 处理器测试共用。

use auction_application::consignment::{
    ActiveHold, CustodyFacts, ListingEquipmentDetails, ListingEquipmentLabel,
    ListingEquipmentLevel, PositionSummary, PrivateListingV2, PrivateListingViewV2,
};
use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::{
    FeeBps, HeldOffMarketReason, ListingEquipmentIdentity, ListingPrice, ListingVersion,
};
use auction_time::Timestamp;
use uuid::Uuid;

fn time() -> Timestamp {
    Timestamp::parse_from_rfc3339("2026-11-21T00:00:00Z").expect("夹具时刻")
}

fn equipment() -> ListingEquipmentDetails {
    ListingEquipmentDetails::new(
        time(),
        ListingEquipmentLabel {
            name: "Fixture Oak Staff".to_owned(),
            alt_name: None,
            category: "Staff".to_owned(),
            level: ListingEquipmentLevel::Assigned(42),
            tradeable: true,
        },
        Vec::new(),
    )
    .expect("夹具装备")
}

fn listing(
    listing_id: Uuid,
    view: PrivateListingViewV2,
    custody: CustodyFacts,
) -> PrivateListingV2 {
    PrivateListingV2 {
        listing_id,
        version: ListingVersion::new(5).expect("版本"),
        identity: ListingEquipmentIdentity::new("main", "42").expect("装备身份"),
        price: ListingPrice::new(101).expect("标价"),
        pricing_version: 1,
        fee_bps: FeeBps::DEFAULT_CONSIGNED,
        created_at: time(),
        updated_at: time(),
        expires_at: time(),
        view,
        custody,
    }
    .validated()
    .expect("夹具形状")
}

fn position(state: PositionState) -> PositionSummary {
    PositionSummary {
        position_id: Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0a01),
        state,
        version: 2,
        generation: Some(1),
    }
}

/// 已代管并上架、没有撤回意愿也没有止售的挂单。
pub(crate) fn available(listing_id: Uuid) -> PrivateListingV2 {
    listing(
        listing_id,
        PrivateListingViewV2::Available(equipment()),
        CustodyFacts {
            position: Some(position(PositionState::InCustody)),
            hold: None,
            withdrawal_intent_registered: false,
            loss_case_id: None,
            relist_allowed: false,
        },
    )
}

/// 在库下架且带有效运营止售的挂单。
pub(crate) fn held_by_operations(listing_id: Uuid, hold_id: Uuid) -> PrivateListingV2 {
    listing(
        listing_id,
        PrivateListingViewV2::HeldOffMarket {
            equipment: equipment(),
            reason: HeldOffMarketReason::OperationsHold,
        },
        CustodyFacts {
            position: Some(position(PositionState::InCustody)),
            hold: Some(ActiveHold { hold_id, set_at: time() }),
            withdrawal_intent_registered: false,
            loss_case_id: None,
            relist_allowed: false,
        },
    )
}
