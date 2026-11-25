//! 寄售资金端口回归（核心：命令幂等、opening 份额、阻断、手续费冲回只冲本笔），用内存假实现。

use std::future::Future;
use std::task::{Context, Poll, Waker};

use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::{BlockedLeg, FundingMode, OpeningBlock, OpeningSlice};
use auction_domain::consignment::{FeeBps, ListingPrice, quote_v1};
use auction_time::Timestamp;
use uuid::Uuid;

use super::test_support::FakeFunding;
use super::{
    CertifyCod, CodLeg, ConsignmentFundingPreflight, ConsignmentFundingStore, ConvertRefund,
    DispatchFreeze, FreshCashObservation, FundingPreflightFact, FundingUnavailable, ReceiptKey,
    RegisterOpeningAttribution,
};
use crate::ports::settlement::{FundingError, FundingMutation};

fn run<T>(future: impl Future<Output = T>) -> T {
    match std::pin::pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("假实现的 future 必须立即就绪"),
    }
}

fn id(byte: u8) -> Uuid {
    Uuid::from_bytes([byte; 16])
}

fn mutation(byte: u8, digest: u8) -> FundingMutation {
    FundingMutation { command_id: id(byte), request_digest: [digest; 32] }
}

fn key(uid: &str, mail: &str) -> ReceiptKey {
    ReceiptKey { uid: uid.to_owned(), payment_mail_id: mail.to_owned(), leg: CodLeg::Carrier }
}

fn certify(
    mutation: FundingMutation,
    contract: u8,
    receipt: ReceiptKey,
    price: i64,
    attribution: Option<Uuid>,
) -> CertifyCod {
    let price = ListingPrice::new(price).unwrap();
    CertifyCod {
        mutation,
        contract_id: id(contract),
        payment_obligation_id: id(contract + 100),
        original_delivery_id: id(contract + 50),
        receipt,
        world: SettlementWorld::Isekai,
        mode: FundingMode::StrictCash,
        price,
        quote: quote_v1(price, FeeBps::DEFAULT_CONSIGNED),
        gross: price.value() + 1,
        freeze: DispatchFreeze {
            prepare_digest: [1; 32],
            activation_digest: [2; 32],
            body_digest: [3; 32],
            equipment_ids: Vec::new(),
        },
        cash: Some(FreshCashObservation {
            uid: "uid-a".to_owned(),
            credits: 1_000,
            observed_at: Timestamp::default(),
            expires_at: Timestamp::default(),
        }),
        attribution_id: attribution,
    }
}

fn slice(entry: u8, start: i64, end: i64) -> OpeningSlice {
    OpeningSlice { entry_id: id(entry), start, end }
}

fn register(
    fake: &FakeFunding,
    command: u8,
    receipt: ReceiptKey,
    slices: Vec<OpeningSlice>,
) -> Result<super::AttributionReceipt, FundingError> {
    run(fake.register_opening_attribution(RegisterOpeningAttribution {
        mutation: mutation(command, command),
        receipt,
        slices,
    }))
}

#[test]
fn certify_issues_only_the_part_not_in_opening() {
    for (included, issued) in [(102, 0), (60, 42), (0, 102)] {
        let fake = FakeFunding::default();
        let receipt = key("uid-a", "mail-1");
        let attribution = (included > 0).then(|| {
            register(&fake, 1, receipt.clone(), vec![slice(9, 0, included)]).unwrap().attribution_id
        });
        let done =
            run(fake.certify_cod(certify(mutation(2, 2), 3, receipt, 101, attribution))).unwrap();
        assert_eq!(done.split.new_issue(), issued);
        assert_eq!(fake.total_issued(), issued);
    }
}

#[test]
fn opening_share_cannot_back_two_receipts() {
    let fake = FakeFunding::default();
    register(&fake, 1, key("uid-a", "mail-1"), vec![slice(9, 0, 60)]).unwrap();
    // 另一经济实收抢同一份额：冲突。
    let stolen = register(&fake, 2, key("uid-a", "mail-2"), vec![slice(9, 30, 90)]);
    assert!(matches!(stolen, Err(FundingError::Conflict)));
    // 同 entry 的不相交区间可以并存。
    assert!(register(&fake, 3, key("uid-a", "mail-2"), vec![slice(9, 60, 90)]).is_ok());
}

#[test]
fn certify_replay_never_issues_twice() {
    let fake = FakeFunding::default();
    let receipt = key("uid-a", "mail-1");
    let first =
        run(fake.certify_cod(certify(mutation(2, 2), 3, receipt.clone(), 101, None))).unwrap();
    assert!(!first.replayed);
    // 同命令重放、换命令复见同一实收：都读原结果，发行额不变。
    let again =
        run(fake.certify_cod(certify(mutation(2, 2), 3, receipt.clone(), 101, None))).unwrap();
    let reseen =
        run(fake.certify_cod(certify(mutation(4, 4), 3, receipt.clone(), 101, None))).unwrap();
    assert!(again.replayed && reseen.replayed);
    assert_eq!(fake.total_issued(), 102);
    // 同命令 ID 异请求，以及同实收异价格，均冲突。
    let same_id_other_request =
        run(fake.certify_cod(certify(mutation(2, 9), 3, receipt.clone(), 101, None)));
    assert!(matches!(same_id_other_request, Err(FundingError::Conflict)));
    let other_price = run(fake.certify_cod(certify(mutation(5, 5), 3, receipt, 120, None)));
    assert!(matches!(other_price, Err(FundingError::Conflict)));
    assert_eq!(fake.total_issued(), 102);
}

#[test]
fn cash_return_block_stops_opening_of_same_uid_only() {
    let fake = FakeFunding::default();
    let block = OpeningBlock::new(
        "uid-a".to_owned(),
        id(7),
        BlockedLeg::Refund,
        id(8),
        101,
        FundingMode::StrictCash,
    )
    .unwrap();
    fake.seed_block(block);
    let blocked = register(&fake, 2, key("uid-a", "mail-1"), vec![slice(9, 0, 10)]);
    assert!(matches!(blocked, Err(FundingError::Unavailable)));
    // 别的 UID 不受影响。
    assert!(register(&fake, 3, key("uid-b", "mail-9"), vec![slice(8, 0, 10)]).is_ok());
}

#[test]
fn refund_reverses_only_this_contracts_fee_once() {
    let fake = FakeFunding::default();
    let (a, b) = (3, 4);
    for (contract, mail) in [(a, "mail-a"), (b, "mail-b")] {
        run(fake.certify_cod(certify(
            mutation(contract, contract),
            contract,
            key("uid-a", mail),
            101,
            None,
        )))
        .unwrap();
    }
    assert_eq!(fake.fee_rows(), (2, 0));
    let price = ListingPrice::new(101).unwrap();
    let quote = quote_v1(price, FeeBps::DEFAULT_CONSIGNED);
    let convert = |command: u8, contract: u8| ConvertRefund {
        mutation: mutation(command, command),
        contract_id: id(contract),
        refund_obligation_id: id(command),
        price,
        quote,
    };
    let done = run(fake.convert_refund(convert(20, a))).unwrap();
    let reversal = done.fee_reversal.unwrap();
    assert_eq!((reversal.contract_id, reversal.amount), (id(a), 5));
    assert_eq!(done.conversion.to_refund, 101);
    // 只冲了 a 的那一笔，b 仍保持 earned；再次对 a 转换（新命令）不会二次冲回。
    assert_eq!(fake.fee_rows(), (2, 1));
    assert!(matches!(run(fake.convert_refund(convert(21, a))), Err(FundingError::Conflict)));
    assert_eq!(fake.fee_rows(), (2, 1));
    // 同命令重放读原结果。
    assert!(run(fake.convert_refund(convert(20, a))).unwrap().replayed);
    // 没有 earned 的合同不能凭空冲回。
    assert!(run(fake.convert_refund(convert(22, 99))).is_err());
}

#[test]
fn preflight_reports_configured_fact_and_fails_closed_when_unset() {
    let fake = FakeFunding::default();
    assert!(matches!(run(fake.preflight(SettlementWorld::Main)), Err(FundingError::Unproven)));
    let blocked = FundingPreflightFact::Unavailable(FundingUnavailable::OpeningBlocked);
    fake.set_preflight(blocked.clone());
    assert_eq!(run(fake.preflight(SettlementWorld::Main)).unwrap(), blocked);
    let ready = FundingPreflightFact::Ready {
        uid: "uid-a".to_owned(),
        mode: FundingMode::StrictCash,
        boundary_at: Timestamp::default(),
        boundary_source_id: None,
        account_version: 1,
    };
    fake.set_preflight(ready.clone());
    assert_eq!(run(fake.preflight(SettlementWorld::Main)).unwrap(), ready);
}

#[test]
fn certify_rejects_world_and_leg_mismatch() {
    let fake = FakeFunding::default();
    // isekai 世界配原件腿：载体金额会记到原件收据上，必须拒绝且不发行。
    let mut wrong = certify(mutation(2, 2), 3, key("uid-a", "mail-1"), 101, None);
    wrong.receipt.leg = CodLeg::Original;
    assert!(matches!(run(fake.certify_cod(wrong)), Err(FundingError::Invalid)));
    // main 世界配载体腿同样拒绝。
    let mut main_carrier = certify(mutation(3, 3), 4, key("uid-a", "mail-2"), 101, None);
    main_carrier.world = SettlementWorld::Main;
    main_carrier.gross = 101;
    assert!(matches!(run(fake.certify_cod(main_carrier)), Err(FundingError::Invalid)));
    assert_eq!(fake.total_issued(), 0);
    // main 配原件腿通过，按 P 发行。
    let mut right = certify(mutation(4, 4), 5, key("uid-a", "mail-3"), 101, None);
    right.world = SettlementWorld::Main;
    right.receipt.leg = CodLeg::Original;
    right.gross = 101;
    assert!(run(fake.certify_cod(right)).is_ok());
    assert_eq!(fake.total_issued(), 101);
}

#[test]
fn certify_requires_cash_observation_exactly_in_strict_mode() {
    let fake = FakeFunding::default();
    // 严格现金缺观察、观察属于别的 UID，或 record_only 却带观察，都不发行。
    let mut missing = certify(mutation(2, 2), 3, key("uid-a", "mail-1"), 101, None);
    missing.cash = None;
    assert!(matches!(run(fake.certify_cod(missing)), Err(FundingError::Invalid)));
    let mut foreign = certify(mutation(3, 3), 4, key("uid-a", "mail-2"), 101, None);
    foreign.cash.as_mut().unwrap().uid = "uid-b".to_owned();
    assert!(matches!(run(fake.certify_cod(foreign)), Err(FundingError::Invalid)));
    let mut observed = certify(mutation(4, 4), 5, key("uid-a", "mail-3"), 101, None);
    observed.mode = FundingMode::RecordOnly;
    assert!(matches!(run(fake.certify_cod(observed)), Err(FundingError::Invalid)));
    assert_eq!(fake.total_issued(), 0);
}

#[test]
fn convert_refund_rejects_unbalanced_quote_without_reversing_fee() {
    let fake = FakeFunding::default();
    run(fake.certify_cod(certify(mutation(3, 3), 3, key("uid-a", "mail-1"), 101, None))).unwrap();
    let price = ListingPrice::new(101).unwrap();
    let mut quote = quote_v1(price, FeeBps::DEFAULT_CONSIGNED);
    quote.seller_amount += 1;
    let bad = ConvertRefund {
        mutation: mutation(20, 20),
        contract_id: id(3),
        refund_obligation_id: id(20),
        price,
        quote,
    };
    assert!(matches!(run(fake.convert_refund(bad)), Err(FundingError::Invalid)));
    assert_eq!(fake.fee_rows(), (1, 0));
}
