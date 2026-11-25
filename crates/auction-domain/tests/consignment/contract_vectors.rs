//! 寄售合同的金额守恒、义务图形状、义务 ID 冻结向量与撤回无金额的回归。
//!
//! 只使用公开领域 API；义务 ID 向量由独立实现（Python 的 SHA-256 长度前缀算法）核算后冻结，
//! 任何派生算法或 namespace 变化都必须让它们失败。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_domain::consignment::{
    ConsignmentContract, ConsignmentContractCore, ConsignmentObligationGraph,
    ConsignmentSalePolicy, FeeBps, FundingAccountingMode, ListingPrice, ListingQuote,
    ListingVersion, SaleFundingFreeze, SaleTerms, SaleTermsInput, WithdrawalTerms,
    build_consignment_obligations, consignment_obligation_id, validate_consignment_graph,
};
use auction_domain::{
    CommandId, ConsignmentContractId, SettlementAmount, SettlementObligationKind,
    SettlementObligationState, SettlementWorld, UserId,
};
use auction_time::Timestamp;
use uuid::Uuid;

pub(super) fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("测试夹具必须合法: {error:?}"))
}

fn uuid(value: &str) -> Uuid {
    must(Uuid::parse_str(value))
}

fn at(day: u32) -> Timestamp {
    Timestamp::from_ymd_hms(2026, 11, day, 0, 0, 0).unwrap()
}

fn core(world: SettlementWorld) -> ConsignmentContractCore {
    ConsignmentContractCore {
        contract_id: ConsignmentContractId::from_uuid(uuid("7b0c1d2e-3f40-4a51-8b62-9c73d4e5f607")),
        origin_command_id: CommandId::from_uuid(uuid("11111111-1111-4111-8111-111111111111")),
        listing_id: uuid("22222222-2222-4222-8222-222222222222"),
        position_id: uuid("33333333-3333-4333-8333-333333333333"),
        generation: 1,
        custody_equipment_id: uuid("44444444-4444-4444-8444-444444444444"),
        world,
        canonical_eid: "123456".to_owned(),
        season_window_id: (world == SettlementWorld::Isekai)
            .then(|| uuid("55555555-5555-4555-8555-555555555555")),
        seller_user_id: UserId::from_uuid(uuid("66666666-6666-4666-8666-666666666666")),
        frozen_seller_identity: "Seller".to_owned(),
        created_at: at(1),
    }
}

fn sale_terms(price: i64, bps: i64) -> SaleTerms {
    must(SaleTerms::new(SaleTermsInput {
        buyer_user_id: UserId::from_uuid(uuid("77777777-7777-4777-8777-777777777777")),
        frozen_buyer_identity: "Buyer".to_owned(),
        listing_version: must(ListingVersion::new(3)),
        price: must(ListingPrice::new(price)),
        fee_bps: must(FeeBps::new(bps)),
        payment_deadline_at: at(8),
        policy: must(ConsignmentSalePolicy::new(7, 3, 5)),
        funding: SaleFundingFreeze {
            uid: "bot-uid".to_owned(),
            mode: FundingAccountingMode::StrictCash,
            boundary_at: at(1),
            boundary_source_id: None,
            account_version: 0,
        },
    }))
}

pub(super) fn sale(world: SettlementWorld, price: i64, bps: i64) -> ConsignmentContract {
    ConsignmentContract::Sale { core: core(world), terms: sale_terms(price, bps) }
}

pub(super) fn withdrawal(world: SettlementWorld) -> ConsignmentContract {
    ConsignmentContract::Withdrawal {
        core: core(world),
        terms: WithdrawalTerms { intent_id: None, predecessor_contract_id: None },
    }
}

pub(super) fn graph(contract: &ConsignmentContract) -> ConsignmentObligationGraph {
    must(build_consignment_obligations(contract))
}

#[test]
fn sale_terms_conserve_credits_with_floored_fee() {
    // F = floor(P * bps / 10^4)，S = P - F；奇数价格向下取整的差额归卖家。
    for (price, bps, fee, seller) in [(101, 500, 5, 96), (119, 500, 5, 114), (19, 0, 0, 19)] {
        let terms = sale_terms(price, bps);
        assert_eq!(terms.quote, ListingQuote { fee, seller_amount: seller });
        assert_eq!(terms.quote.fee + terms.quote.seller_amount, price);
        let payout = graph(&sale(SettlementWorld::Main, price, bps)).obligations[1].amount;
        assert_eq!(payout.value(), seller);
    }
}

#[test]
fn tampered_sale_quote_is_rejected_instead_of_corrected() {
    let mut terms = sale_terms(101, 500);
    terms.quote = ListingQuote { fee: 4, seller_amount: 97 };
    assert!(terms.validate().is_err());
    let contract = ConsignmentContract::Sale { core: core(SettlementWorld::Main), terms };
    assert!(build_consignment_obligations(&contract).is_err());
}

#[test]
fn graph_shapes_are_fixed_per_contract_kind() {
    use SettlementObligationKind::{
        BuyerRefund, SellerPayout, WinnerEquipmentCod, WinnerEquipmentHandoff,
        WinnerPaymentCarrierCod, WithdrawnReturn,
    };
    use SettlementObligationState::{Blocked, Contingent, Ready};
    let shape = |contract: &ConsignmentContract| {
        let graph = graph(contract);
        assert!(validate_consignment_graph(&graph).is_ok());
        graph
            .obligations
            .iter()
            .map(|item| (item.kind, item.state, item.amount.value(), item.prerequisite_id))
            .collect::<Vec<_>>()
    };

    let main = graph(&sale(SettlementWorld::Main, 101, 500));
    let (cod, payout, refund) =
        (main.obligations[0].id, main.obligations[1].id, main.obligations[2].id);
    assert_eq!(
        shape(&sale(SettlementWorld::Main, 101, 500)),
        vec![
            (WinnerEquipmentCod, Ready, 101, None),
            (SellerPayout, Blocked, 96, Some(cod)),
            (BuyerRefund, Contingent, 101, Some(cod)),
        ]
    );
    assert_ne!(payout, refund);

    let isekai = graph(&sale(SettlementWorld::Isekai, 101, 500));
    let (carrier, handoff) = (isekai.obligations[0].id, isekai.obligations[1].id);
    assert_eq!(isekai.obligations[0].carrier_credit, 1);
    assert_eq!(
        shape(&sale(SettlementWorld::Isekai, 101, 500)),
        vec![
            (WinnerPaymentCarrierCod, Ready, 101, None),
            (WinnerEquipmentHandoff, Blocked, 0, Some(carrier)),
            (SellerPayout, Blocked, 96, Some(handoff)),
            (BuyerRefund, Contingent, 101, Some(carrier)),
        ]
    );

    assert_eq!(shape(&withdrawal(SettlementWorld::Main)), vec![(WithdrawnReturn, Ready, 0, None)]);
    assert_eq!(graph(&withdrawal(SettlementWorld::Isekai)).obligations.len(), 1);
}

#[test]
fn any_tampering_with_the_graph_is_rejected() {
    for contract in [
        sale(SettlementWorld::Main, 101, 500),
        sale(SettlementWorld::Isekai, 101, 500),
        withdrawal(SettlementWorld::Main),
    ] {
        let base = graph(&contract);
        let mut variants: Vec<ConsignmentObligationGraph> = Vec::new();
        let mut missing = base.clone();
        missing.obligations.pop();
        variants.push(missing);
        let mut extra = base.clone();
        extra.obligations.push(base.obligations[0].clone());
        variants.push(extra);
        let mut amount = base.clone();
        amount.obligations[0].amount =
            SettlementAmount::new(amount.obligations[0].amount.value() + 1).unwrap();
        variants.push(amount);
        let mut recipient = base.clone();
        recipient.obligations[0].recipient_game_identity = "Other".to_owned();
        variants.push(recipient);
        let mut equipment = base.clone();
        equipment.obligations[0].custody_equipment_id = Uuid::from_u128(9);
        variants.push(equipment);
        if base.obligations.len() > 1 {
            let mut reordered = base.clone();
            reordered.obligations.swap(0, 1);
            variants.push(reordered);
            let mut prerequisite = base.clone();
            prerequisite.obligations[1].prerequisite_id = None;
            variants.push(prerequisite);
            let mut state = base.clone();
            state.obligations[1].state = SettlementObligationState::Ready;
            variants.push(state);
        }
        for (index, variant) in variants.iter().enumerate() {
            assert!(validate_consignment_graph(variant).is_err(), "变体 {index} 必须被拒");
        }
        assert!(validate_consignment_graph(&base).is_ok());
    }
}

#[test]
fn withdrawal_with_any_amount_is_rejected() {
    let base = graph(&withdrawal(SettlementWorld::Main));
    let mut priced = base.clone();
    priced.obligations[0].amount = SettlementAmount::new(1).unwrap();
    assert!(validate_consignment_graph(&priced).is_err());
    // 把成交腿塞进撤回图同样被拒：撤回合同没有任何金额来源。
    let mut with_payout = base.clone();
    let sale_graph = graph(&sale(SettlementWorld::Main, 101, 500));
    with_payout.obligations.push(sale_graph.obligations[1].clone());
    assert!(validate_consignment_graph(&with_payout).is_err());
    assert_eq!(base.obligations[0].amount, SettlementAmount::zero());
}

#[test]
fn main_contract_cannot_carry_a_season_window() {
    let mut contract = sale(SettlementWorld::Main, 101, 500);
    if let ConsignmentContract::Sale { core, .. } = &mut contract {
        core.season_window_id = Some(uuid("55555555-5555-4555-8555-555555555555"));
    }
    assert!(build_consignment_obligations(&contract).is_err());
}

#[test]
fn obligation_ids_match_frozen_vectors() {
    let contract_id =
        ConsignmentContractId::from_uuid(uuid("7b0c1d2e-3f40-4a51-8b62-9c73d4e5f607"));
    for (kind, expected) in [
        (SettlementObligationKind::WinnerEquipmentCod, "1c1fc98e-ce68-5c41-a7e9-d51921836b43"),
        (SettlementObligationKind::WinnerPaymentCarrierCod, "edc04c47-ae22-5666-8ef4-0c420fcabd90"),
        (SettlementObligationKind::WinnerEquipmentHandoff, "050481e8-fcf5-5c1a-9578-cc9633cf285f"),
        (SettlementObligationKind::SellerPayout, "718753dc-66d5-526e-b368-7c6d5fc3d8cd"),
        (SettlementObligationKind::BuyerRefund, "5a08e589-b5e9-5259-90b8-156ed65c5410"),
        (SettlementObligationKind::WithdrawnReturn, "b7d9324b-f5d0-5031-8cd0-d6af05cff30c"),
    ] {
        assert_eq!(
            consignment_obligation_id(contract_id, kind).as_uuid(),
            uuid(expected),
            "{kind:?}"
        );
    }
    // 同合同重复构图得同 ID；不同合同得不同 ID。
    let first = graph(&sale(SettlementWorld::Main, 101, 500));
    let again = graph(&sale(SettlementWorld::Main, 101, 500));
    assert_eq!(first, again);
    assert_eq!(first.obligations[0].id.as_uuid(), uuid("1c1fc98e-ce68-5c41-a7e9-d51921836b43"));
    let other = consignment_obligation_id(
        ConsignmentContractId::from_uuid(Uuid::from_u128(1)),
        SettlementObligationKind::WinnerEquipmentCod,
    );
    assert_ne!(other, first.obligations[0].id);
}
