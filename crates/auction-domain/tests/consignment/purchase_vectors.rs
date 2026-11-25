//! 购买判定的冻结向量：拒绝码与状态、金额守恒、季界等号与付款期限 D0。
//!
//! 只使用公开领域 API。码表与状态码随账本回执保存，字节不得再改；金额与期限向量由独立
//! 算术核算后写成字面量。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_domain::consignment::deposit::PositionState;
use auction_domain::consignment::purchase::{
    PurchaseFacts, PurchaseFunding, PurchaseInput, PurchaseListing, PurchaseParty,
    PurchasePosition, PurchaseRejection, PurchaseSaleInput, build_purchase_sale_terms,
    decide_purchase, payment_deadline_at, verify_payment_deadline,
};
use auction_domain::consignment::{
    CalendarVersion, ConsignmentSalePolicy, FeeBps, FrozenSeason, FundingAccountingMode,
    ListingMode, ListingPrice, ListingState, ListingVersion, PRICING_VERSION, SaleFundingFreeze,
    SeasonState, season_state,
};
use auction_domain::{SettlementWorld, UserId};
use auction_time::{Duration, Timestamp};
use uuid::Uuid;

fn at(day: u32, hour: u32) -> Timestamp {
    Timestamp::from_ymd_hms(2026, 11, day, hour, 0, 0).unwrap()
}

fn input(price: i64) -> PurchaseInput {
    PurchaseInput {
        listing_id: Uuid::from_u128(1),
        expected_version: ListingVersion::new(4).unwrap(),
        expected_price: ListingPrice::new(price).unwrap(),
    }
}

fn facts(world: SettlementWorld, price: i64, bps: i64) -> PurchaseFacts {
    let party = |n: u128| PurchaseParty {
        user_id: UserId::from_uuid(Uuid::from_u128(n)),
        active: true,
        eligible: true,
        is_authenticated_bot: false,
    };
    PurchaseFacts {
        now: at(20, 12),
        service_enabled: true,
        listing_epoch_current: true,
        listing: PurchaseListing {
            listing_id: Uuid::from_u128(1),
            state: ListingState::Available,
            version: ListingVersion::new(4).unwrap(),
            price: ListingPrice::new(price).unwrap(),
            world,
            mode: ListingMode::Consigned,
            fee_bps: FeeBps::new(bps).unwrap(),
            pricing_version: PRICING_VERSION,
        },
        position: Some(PurchasePosition {
            position_id: Uuid::from_u128(2),
            state: PositionState::InCustody,
            generation: Some(1),
            generation_consumed: false,
            take_proven: true,
        }),
        buyer: party(10),
        seller: party(20),
        season: match world {
            SettlementWorld::Main => SeasonState::NotSeasonal,
            SettlementWorld::Isekai => SeasonState::Open,
        },
        active_hold: false,
        buyer_sanction_pending: false,
        opposite_unknown_write: false,
        unclosed_delivery: false,
        funding: PurchaseFunding::Ready(SaleFundingFreeze {
            uid: "42".to_owned(),
            mode: FundingAccountingMode::RecordOnly,
            boundary_at: at(1, 0),
            boundary_source_id: None,
            account_version: 0,
        }),
    }
}

#[test]
fn rejection_codes_and_statuses_are_frozen() {
    let table: [(PurchaseRejection, &str, u16); 11] = [
        (PurchaseRejection::ListingNotActive, "listing_not_active", 409),
        (PurchaseRejection::OperationsHold, "operations_hold", 409),
        (PurchaseRejection::StaleVersion, "stale_version", 409),
        (PurchaseRejection::PriceChanged, "price_changed", 409),
        (PurchaseRejection::EquipmentCommitted, "equipment_committed", 409),
        (PurchaseRejection::CustodyUnproven, "custody_unproven", 409),
        (PurchaseRejection::SeasonClosed, "season_closed", 409),
        (PurchaseRejection::BotParty, "bot_party", 403),
        (PurchaseRejection::SelfPurchase, "self_purchase", 403),
        (PurchaseRejection::ParticipantIneligible, "participant_ineligible", 403),
        (PurchaseRejection::FundingUnavailable, "funding_unavailable", 503),
    ];
    // 表序即判定优先序，ALL 必须与之逐项一致。
    assert_eq!(PurchaseRejection::ALL.len(), table.len());
    for (index, (rejection, code, status)) in table.into_iter().enumerate() {
        assert_eq!(PurchaseRejection::ALL[index], rejection);
        assert_eq!(rejection.code(), code);
        assert_eq!(rejection.http_status(), status);
    }
}

#[test]
fn grant_amounts_match_frozen_vectors_and_conserve_credits() {
    // (标价, 基点, 手续费 F, 卖家所得 S)
    let vectors = [
        (101, 500, 5, 96),
        (119, 500, 5, 114),
        (120, 500, 6, 114),
        (19, 500, 0, 19),
        (1, 9_999, 0, 1),
        (1_000_000_000, 9_999, 999_900_000, 100_000),
        (1_000_000_000, 0, 0, 1_000_000_000),
    ];
    for (price, bps, fee, seller) in vectors {
        let grant =
            decide_purchase(&input(price), &facts(SettlementWorld::Main, price, bps)).unwrap();
        assert_eq!((grant.quote.fee, grant.quote.seller_amount), (fee, seller), "{price}/{bps}");
        assert_eq!(grant.quote.fee + grant.quote.seller_amount, price);
    }
    // 反例：越界标价与费率在构造处失败关闭，不会进入判定。
    assert!(ListingPrice::new(0).is_err());
    assert!(ListingPrice::new(1_000_000_001).is_err());
    assert!(FeeBps::new(10_000).is_err());
    assert!(FeeBps::new(-1).is_err());
}

fn isekai_season() -> FrozenSeason {
    FrozenSeason {
        world: SettlementWorld::Isekai,
        season_id: "2026-09-01".to_owned(),
        start_utc: at(1, 0),
        end_utc: at(28, 0),
        admission_cutoff_utc: at(27, 0),
        calendar_version: CalendarVersion::from_bytes([7; 32]),
        preclose_days: 1,
    }
}

fn purchase_at(now: Timestamp) -> Result<(), PurchaseRejection> {
    let season = isekai_season();
    let mut facts = facts(SettlementWorld::Isekai, 101, 500);
    facts.now = now;
    facts.season = season_state(SettlementWorld::Isekai, Some(&season), None, now);
    decide_purchase(&input(101), &facts).map(|_| ())
}

#[test]
fn season_boundaries_use_equality_as_the_closing_point() {
    let one_second = Duration::seconds(1);
    let cutoff = isekai_season().admission_cutoff_utc;
    let end = isekai_season().end_utc;
    // 预停点前一秒仍开放；等于预停点即拒绝；等于季末同样拒绝。
    assert_eq!(purchase_at(cutoff.checked_sub_signed(one_second).unwrap()), Ok(()));
    assert_eq!(purchase_at(cutoff), Err(PurchaseRejection::SeasonClosed));
    assert_eq!(
        purchase_at(end.checked_sub_signed(one_second).unwrap()),
        Err(PurchaseRejection::SeasonClosed)
    );
    assert_eq!(purchase_at(end), Err(PurchaseRejection::SeasonClosed));
    // 季前同样失败关闭（统筹方裁定：季前保持 unknown，不授权）。
    let start = isekai_season().start_utc;
    assert_eq!(
        purchase_at(start.checked_sub_signed(one_second).unwrap()),
        Err(PurchaseRejection::SeasonClosed)
    );
    assert_eq!(purchase_at(start), Ok(()));
}

fn sale_input(committed_at: Timestamp, days: u16) -> PurchaseSaleInput {
    PurchaseSaleInput {
        committed_at,
        buyer_user_id: UserId::from_uuid(Uuid::from_u128(10)),
        frozen_buyer_identity: "Buyer".to_owned(),
        listing_version: ListingVersion::new(4).unwrap(),
        grant: decide_purchase(&input(101), &facts(SettlementWorld::Main, 101, 500)).unwrap(),
        policy: ConsignmentSalePolicy::new(days, 3, 5).unwrap(),
    }
}

#[test]
fn payment_deadline_vectors() {
    // (冻结天数, 期望的 D0)：成交时刻 2026-11-20T12:00:00Z。
    for (days, expected) in [(1, at(21, 12)), (7, at(27, 12)), (10, at(30, 12))] {
        let terms = build_purchase_sale_terms(sale_input(at(20, 12), days)).unwrap();
        assert_eq!(terms.payment_deadline_at, expected, "{days} 天");
        assert_eq!(payment_deadline_at(at(20, 12), &terms.policy).unwrap(), expected);
        assert!(verify_payment_deadline(at(20, 12), &terms).is_ok());
    }
    // 最大范围 90 天：2026-11-20T12:00 + 90 天 = 2027-02-18T12:00。
    let max = build_purchase_sale_terms(sale_input(at(20, 12), 90)).unwrap();
    assert_eq!(max.payment_deadline_at, Timestamp::from_ymd_hms(2027, 2, 18, 12, 0, 0).unwrap());
    // 反例：同一份条款在另一成交时刻下不再是合法 D0。
    assert!(verify_payment_deadline(at(21, 12), &max).is_err());
}
