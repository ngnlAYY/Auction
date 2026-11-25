//! 购买判定的核心规则测试：每个拒绝条件一正一反，优先序、D0 派生与改价各有正反例。

use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use super::{
    PurchaseFacts, PurchaseFunding, PurchaseInput, PurchaseListing, PurchaseParty,
    PurchasePosition, PurchaseRejection, PurchaseSaleInput, RepriceDecision, RepriceFacts,
    RepriceInput, RepriceRejection, build_purchase_sale_terms, decide_purchase,
    decide_reprice_available, payment_deadline_at, verify_payment_deadline,
};
use crate::consignment::deposit::PositionState;
use crate::consignment::{
    ConsignmentSalePolicy, FeeBps, FundingAccountingMode, ListingMode, ListingPrice, ListingState,
    ListingVersion, PRICING_VERSION, SaleFundingFreeze, SeasonState,
};
use crate::{SettlementWorld, UserId};

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn now() -> Timestamp {
    Timestamp::from_ymd_hms(2026, 11, 20, 12, 0, 0).expect("测试时刻")
}

fn party(n: u128) -> PurchaseParty {
    PurchaseParty {
        user_id: UserId::from_uuid(id(n)),
        active: true,
        eligible: true,
        is_authenticated_bot: false,
    }
}

fn input() -> PurchaseInput {
    PurchaseInput {
        listing_id: id(1),
        expected_version: ListingVersion::new(4).expect("测试版本"),
        expected_price: ListingPrice::new(101).expect("测试价格"),
    }
}

/// 全部前置满足的 main 世界快照。
fn facts() -> PurchaseFacts {
    PurchaseFacts {
        now: now(),
        service_enabled: true,
        listing_epoch_current: true,
        listing: PurchaseListing {
            listing_id: id(1),
            state: ListingState::Available,
            version: ListingVersion::new(4).expect("测试版本"),
            price: ListingPrice::new(101).expect("测试价格"),
            world: SettlementWorld::Main,
            mode: ListingMode::Consigned,
            fee_bps: FeeBps::DEFAULT_CONSIGNED,
            pricing_version: PRICING_VERSION,
        },
        position: Some(PurchasePosition {
            position_id: id(2),
            state: PositionState::InCustody,
            generation: Some(1),
            generation_consumed: false,
            take_proven: true,
        }),
        buyer: party(10),
        seller: party(20),
        season: SeasonState::NotSeasonal,
        active_hold: false,
        buyer_sanction_pending: false,
        opposite_unknown_write: false,
        unclosed_delivery: false,
        funding: PurchaseFunding::Ready(freeze()),
    }
}

fn freeze() -> SaleFundingFreeze {
    SaleFundingFreeze {
        uid: "42".to_owned(),
        mode: FundingAccountingMode::StrictCash,
        boundary_at: now(),
        boundary_source_id: None,
        account_version: 0,
    }
}

fn reject(mutate: impl FnOnce(&mut PurchaseFacts)) -> PurchaseRejection {
    let mut facts = facts();
    mutate(&mut facts);
    decide_purchase(&input(), &facts).expect_err("应被拒绝")
}

fn position(facts: &mut PurchaseFacts) -> &mut PurchasePosition {
    facts.position.as_mut().expect("测试 position")
}

#[test]
fn baseline_grants_with_recomputed_quote() {
    let grant = decide_purchase(&input(), &facts()).expect("基线应成交");
    assert_eq!((grant.quote.fee, grant.quote.seller_amount), (5, 96));
    assert_eq!(grant.quote.fee + grant.quote.seller_amount, grant.price.value());
    assert_eq!(grant.listing_version_after.value(), 5);
    assert_eq!(grant.generation, 1);
}

#[test]
fn listing_must_be_open_consigned_available() {
    assert_eq!(
        reject(|f| f.listing.state = ListingState::HeldOffMarket),
        PurchaseRejection::ListingNotActive
    );
    assert_eq!(
        reject(|f| f.listing.state = ListingState::SalePending),
        PurchaseRejection::ListingNotActive
    );
    assert_eq!(
        reject(|f| f.listing.mode = ListingMode::Direct),
        PurchaseRejection::ListingNotActive
    );
    assert_eq!(reject(|f| f.service_enabled = false), PurchaseRejection::ListingNotActive);
    assert_eq!(reject(|f| f.listing_epoch_current = false), PurchaseRejection::ListingNotActive);
}

#[test]
fn operations_hold_blocks_purchase() {
    assert_eq!(reject(|f| f.active_hold = true), PurchaseRejection::OperationsHold);
}

#[test]
fn version_and_price_must_match_exactly() {
    assert_eq!(
        reject(|f| f.listing.version = ListingVersion::new(5).expect("版本")),
        PurchaseRejection::StaleVersion
    );
    assert_eq!(
        reject(|f| f.listing.price = ListingPrice::new(102).expect("价格")),
        PurchaseRejection::PriceChanged
    );
}

#[test]
fn committed_or_unclosed_equipment_is_rejected_but_closed_predecessor_is_kept() {
    assert_eq!(
        reject(|f| position(f).generation_consumed = true),
        PurchaseRejection::EquipmentCommitted
    );
    assert_eq!(
        reject(|f| position(f).state = PositionState::Committed),
        PurchaseRejection::EquipmentCommitted
    );
    assert_eq!(
        reject(|f| position(f).state = PositionState::InTransit),
        PurchaseRejection::EquipmentCommitted
    );
    assert_eq!(reject(|f| f.opposite_unknown_write = true), PurchaseRejection::EquipmentCommitted);
    assert_eq!(reject(|f| f.unclosed_delivery = true), PurchaseRejection::EquipmentCommitted);
    // 反例：前代交割已闭合（当前代无未封闭交割、未消耗）时可成交。
    assert!(decide_purchase(&input(), &facts()).is_ok());
}

#[test]
fn custody_must_be_strictly_proven_and_continuous() {
    assert_eq!(reject(|f| f.position = None), PurchaseRejection::CustodyUnproven);
    assert_eq!(reject(|f| position(f).take_proven = false), PurchaseRejection::CustodyUnproven);
    assert_eq!(
        reject(|f| position(f).state = PositionState::Reconcile),
        PurchaseRejection::CustodyUnproven
    );
    assert_eq!(
        reject(|f| position(f).state = PositionState::AwaitingTake),
        PurchaseRejection::CustodyUnproven
    );
    assert_eq!(reject(|f| position(f).generation = None), PurchaseRejection::CustodyUnproven);
}

#[test]
fn season_must_be_open_for_isekai_and_absent_for_main() {
    let isekai = |state: SeasonState| {
        let mut f = facts();
        f.listing.world = SettlementWorld::Isekai;
        f.season = state;
        decide_purchase(&input(), &f)
    };
    assert!(isekai(SeasonState::Open).is_ok());
    for state in
        [SeasonState::Preclose, SeasonState::Closed, SeasonState::Unknown, SeasonState::NotSeasonal]
    {
        assert_eq!(isekai(state), Err(PurchaseRejection::SeasonClosed), "{state:?}");
    }
    assert_eq!(reject(|f| f.season = SeasonState::Open), PurchaseRejection::SeasonClosed);
}

#[test]
fn authenticated_bot_cannot_be_either_party() {
    assert_eq!(reject(|f| f.buyer.is_authenticated_bot = true), PurchaseRejection::BotParty);
    assert_eq!(reject(|f| f.seller.is_authenticated_bot = true), PurchaseRejection::BotParty);
}

#[test]
fn same_local_account_cannot_buy_own_listing() {
    assert_eq!(reject(|f| f.buyer = f.seller), PurchaseRejection::SelfPurchase);
    // 反例：只有账号不同才放行，其他字段相同不构成自买。
    let mut other = facts();
    other.buyer = PurchaseParty { user_id: UserId::from_uuid(id(11)), ..other.seller };
    assert!(decide_purchase(&input(), &other).is_ok());
}

#[test]
fn ineligible_party_or_pending_sanction_blocks_purchase() {
    assert_eq!(reject(|f| f.buyer.eligible = false), PurchaseRejection::ParticipantIneligible);
    assert_eq!(reject(|f| f.buyer.active = false), PurchaseRejection::ParticipantIneligible);
    assert_eq!(reject(|f| f.seller.eligible = false), PurchaseRejection::ParticipantIneligible);
    assert_eq!(reject(|f| f.seller.active = false), PurchaseRejection::ParticipantIneligible);
    assert_eq!(
        reject(|f| f.buyer_sanction_pending = true),
        PurchaseRejection::ParticipantIneligible
    );
}

#[test]
fn funding_must_be_ready() {
    assert_eq!(
        reject(|f| f.funding = PurchaseFunding::Unavailable),
        PurchaseRejection::FundingUnavailable
    );
}

#[test]
fn grant_freezes_the_same_funding_fact_that_was_judged() {
    let mut facts = facts();
    let other = SaleFundingFreeze {
        uid: "77".to_owned(),
        mode: FundingAccountingMode::RecordOnly,
        boundary_at: now().checked_add_signed(Duration::minutes(1)).expect("时刻"),
        boundary_source_id: None,
        account_version: 0,
    };
    facts.funding = PurchaseFunding::Ready(other.clone());
    let grant = decide_purchase(&input(), &facts).expect("Ready 应成交");
    assert_eq!(grant.funding, other);
    // Sale 条款里的资金冻结取自授予，而不是另行传入。
    let mut sale = sale_input(now());
    sale.grant = grant;
    let terms = build_purchase_sale_terms(sale).expect("构造 Sale 条款");
    assert_eq!(terms.funding, other);
}

#[test]
fn rejection_priority_follows_the_frozen_order() {
    // 每一步同时叠加后面所有条件，结论必须是当前步的拒绝。
    type Step = (PurchaseRejection, fn(&mut PurchaseFacts));
    let steps: [Step; 11] = [
        (PurchaseRejection::ListingNotActive, |f| f.listing.state = ListingState::Closed),
        (PurchaseRejection::OperationsHold, |f| f.active_hold = true),
        (PurchaseRejection::StaleVersion, |f| {
            f.listing.version = ListingVersion::new(9).expect("版本");
        }),
        (PurchaseRejection::PriceChanged, |f| {
            f.listing.price = ListingPrice::new(7).expect("价格");
        }),
        (PurchaseRejection::EquipmentCommitted, |f| f.unclosed_delivery = true),
        (PurchaseRejection::CustodyUnproven, |f| f.position = None),
        (PurchaseRejection::SeasonClosed, |f| f.season = SeasonState::Closed),
        (PurchaseRejection::BotParty, |f| f.buyer.is_authenticated_bot = true),
        (PurchaseRejection::SelfPurchase, |f| f.buyer.user_id = f.seller.user_id),
        (PurchaseRejection::ParticipantIneligible, |f| f.buyer.eligible = false),
        (PurchaseRejection::FundingUnavailable, |f| f.funding = PurchaseFunding::Unavailable),
    ];
    for (index, (expected, _)) in steps.iter().enumerate() {
        let mut facts = facts();
        for (_, apply) in &steps[index..] {
            apply(&mut facts);
        }
        assert_eq!(decide_purchase(&input(), &facts), Err(*expected));
    }
}

fn policy() -> ConsignmentSalePolicy {
    ConsignmentSalePolicy::new(7, 3, 5).expect("测试策略")
}

fn sale_input(committed_at: Timestamp) -> PurchaseSaleInput {
    PurchaseSaleInput {
        committed_at,
        buyer_user_id: UserId::from_uuid(id(10)),
        frozen_buyer_identity: "Buyer".to_owned(),
        listing_version: input().expected_version,
        grant: decide_purchase(&input(), &facts()).expect("基线应成交"),
        policy: policy(),
    }
}

#[test]
fn payment_deadline_is_commit_time_plus_frozen_days() {
    let terms = build_purchase_sale_terms(sale_input(now())).expect("构造 Sale 条款");
    assert_eq!(
        terms.payment_deadline_at,
        now().checked_add_signed(Duration::days(7)).expect("时刻")
    );
    assert_eq!(payment_deadline_at(now(), &policy()), Ok(terms.payment_deadline_at));
    assert!(verify_payment_deadline(now(), &terms).is_ok());
    // 金额由授予带入，F + S = P。
    assert_eq!(terms.quote.fee + terms.quote.seller_amount, terms.price.value());
}

#[test]
fn payment_deadline_rejects_tampered_or_stale_terms() {
    let mut terms = build_purchase_sale_terms(sale_input(now())).expect("构造 Sale 条款");
    // 反例：期限被外部改写（早一秒、晚一秒、按别的成交时刻）一律不符。
    for shifted in [Duration::seconds(-1), Duration::seconds(1)] {
        let mut tampered = terms.clone();
        tampered.payment_deadline_at =
            terms.payment_deadline_at.checked_add_signed(shifted).expect("时刻");
        assert!(verify_payment_deadline(now(), &tampered).is_err());
    }
    let later = now().checked_add_signed(Duration::minutes(1)).expect("时刻");
    assert!(verify_payment_deadline(later, &terms).is_err());
    // 冻结策略天数变化后，旧期限也必须与新天数不符。
    terms.policy = ConsignmentSalePolicy::new(8, 3, 5).expect("策略");
    assert!(verify_payment_deadline(now(), &terms).is_err());
}

#[test]
fn sale_terms_reject_grant_with_mismatched_amounts() {
    let mut input = sale_input(now());
    input.grant.quote.fee += 1;
    input.grant.quote.seller_amount -= 1;
    assert!(build_purchase_sale_terms(input).is_err());
}

fn reprice_facts() -> RepriceFacts {
    RepriceFacts {
        seller_user_id: facts().seller.user_id,
        listing: facts().listing,
        position: facts().position,
        sale_exists: false,
    }
}

fn reprice_input(price: i64) -> RepriceInput {
    RepriceInput {
        seller_user_id: facts().seller.user_id,
        listing_id: id(1),
        expected_version: ListingVersion::new(4).expect("版本"),
        price: ListingPrice::new(price).expect("价格"),
    }
}

#[test]
fn reprice_is_only_allowed_for_the_listing_seller() {
    assert!(matches!(
        decide_reprice_available(&reprice_input(120), &reprice_facts()),
        Ok(RepriceDecision::Changed { .. })
    ));
    // 非卖家（含买家）一律得 listing_not_found，且先于其他判定，不泄露挂单状态。
    for stranger in [facts().buyer.user_id, UserId::from_uuid(id(99))] {
        let mut input = reprice_input(120);
        input.seller_user_id = stranger;
        let rejection = decide_reprice_available(&input, &reprice_facts()).expect_err("应拒绝");
        assert_eq!(rejection, RepriceRejection::ListingNotFound);
        assert_eq!(rejection.code(), "listing_not_found");
        let mut closed = reprice_facts();
        closed.listing.state = ListingState::Closed;
        assert_eq!(
            decide_reprice_available(&input, &closed),
            Err(RepriceRejection::ListingNotFound)
        );
    }
}

#[test]
fn reprice_is_only_allowed_while_available_and_uncommitted() {
    match decide_reprice_available(&reprice_input(120), &reprice_facts()) {
        Ok(RepriceDecision::Changed { quote, listing_version_after, .. }) => {
            assert_eq!((quote.fee, quote.seller_amount), (6, 114));
            assert_eq!(listing_version_after.value(), 5);
        }
        other => panic!("应改价成功: {other:?}"),
    }
    assert_eq!(
        decide_reprice_available(&reprice_input(101), &reprice_facts()),
        Ok(RepriceDecision::Unchanged)
    );

    let mut sold = reprice_facts();
    sold.listing.state = ListingState::SalePending;
    assert_eq!(
        decide_reprice_available(&reprice_input(120), &sold),
        Err(RepriceRejection::EquipmentCommitted)
    );
    let mut with_sale = reprice_facts();
    with_sale.sale_exists = true;
    assert_eq!(
        decide_reprice_available(&reprice_input(120), &with_sale),
        Err(RepriceRejection::EquipmentCommitted)
    );
    let mut committed = reprice_facts();
    committed.position.as_mut().expect("position").state = PositionState::Committed;
    assert_eq!(
        decide_reprice_available(&reprice_input(120), &committed),
        Err(RepriceRejection::EquipmentCommitted)
    );
    let mut held = reprice_facts();
    held.listing.state = ListingState::HeldOffMarket;
    assert_eq!(
        decide_reprice_available(&reprice_input(120), &held),
        Err(RepriceRejection::ListingNotActive)
    );
    let mut closed = reprice_facts();
    closed.listing.state = ListingState::Closed;
    closed.sale_exists = true;
    assert_eq!(
        decide_reprice_available(&reprice_input(120), &closed),
        Err(RepriceRejection::ListingNotActive)
    );
    let mut stale = reprice_input(120);
    stale.expected_version = ListingVersion::new(3).expect("版本");
    assert_eq!(
        decide_reprice_available(&stale, &reprice_facts()),
        Err(RepriceRejection::StaleVersion)
    );
}
