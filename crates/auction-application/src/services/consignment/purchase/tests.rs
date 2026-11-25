//! 购买用例的核心测试：规范编码黄金向量、摘要对每个字段敏感、客户端校验先于存储、资金预检映射。

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use auction_domain::consignment::purchase::{PurchaseFunding, PurchaseInput, RepriceInput};
use auction_domain::consignment::{
    FundingAccountingMode, ListingPrice, ListingVersion, OrderState, funding::FundingMode,
};
use auction_domain::{CommandId, ConsignmentContractId, SettlementWorld, UserId};
use auction_time::Timestamp;
use uuid::Uuid;

use super::encoding::{canonical_purchase, canonical_reprice_v2, request_hash};
use super::{PurchaseCommandService, purchase_funding};
use crate::ports::consignment::{
    ConsignmentError, ConsignmentFuture, ListingPriceDisposition, PurchaseCommand, PurchaseOutcome,
    PurchaseReceipt, PurchaseStore, RepriceV2Command, RepriceV2Outcome, RepriceV2Receipt,
    RepriceV2Store,
};
use crate::ports::settlement::consignment_funding::{FundingPreflightFact, FundingUnavailable};

fn run<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..4 {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return result;
        }
    }
    panic!("内存假实现未在预期轮数内完成")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn buyer() -> UserId {
    UserId::from_uuid(Uuid::parse_str("77777777-7777-4777-8777-777777777777").expect("UUID"))
}

fn listing() -> Uuid {
    Uuid::parse_str("22222222-2222-4222-8222-222222222222").expect("UUID")
}

fn version(value: u64) -> ListingVersion {
    ListingVersion::new(value).expect("测试版本")
}

fn price(value: i64) -> ListingPrice {
    ListingPrice::new(value).expect("测试价格")
}

/// 向量由独立实现（Python 的 hashlib 与 struct）核算；前缀、字段顺序或字节序变化必须失败。
#[test]
fn canonical_encoding_matches_frozen_vectors() {
    let cases = [
        (
            canonical_purchase(buyer(), listing(), version(3), price(101)),
            "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e76320000000000000000087075726368617365000000000000000b7075626c69635f75736572777777777777477787777777777777772222222222224222822222222222222200000000000000030000000000000065",
            "576d1f39d733f19e9570f26351f926af01c0631b2d4914502decd98254cb6bec",
        ),
        (
            canonical_reprice_v2(buyer(), listing(), version(3), price(119)),
            "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e763200000000000000000772657072696365000000000000000b7075626c69635f75736572777777777777477787777777777777772222222222224222822222222222222200000000000000030000000000000077",
            "e0a26119a713234b800635b716d9cb117e39a6713ed40f086bfbe11b24ec3c82",
        ),
        (
            canonical_purchase(
                buyer(),
                listing(),
                version(ListingVersion::MAX),
                price(1_000_000_000),
            ),
            "61756374696f6e2e636f6e7369676e6d656e742e636f6d6d616e642e76320000000000000000087075726368617365000000000000000b7075626c69635f757365727777777777774777877777777777777722222222222242228222222222222222001fffffffffffff000000003b9aca00",
            "353da8602f3e971c1bd3e7927189ec80cacf177ff95aeca255f36d0e72b56428",
        ),
    ];
    for (bytes, expected_bytes, expected_hash) in cases {
        assert_eq!(hex(&bytes), expected_bytes);
        assert_eq!(hex(&request_hash(&bytes).as_bytes()), expected_hash);
    }
}

#[test]
fn request_hash_changes_with_every_bound_field() {
    let base = canonical_purchase(buyer(), listing(), version(3), price(101));
    let other_buyer = UserId::from_uuid(Uuid::from_u128(1));
    let variants = [
        canonical_purchase(other_buyer, listing(), version(3), price(101)),
        canonical_purchase(buyer(), Uuid::from_u128(2), version(3), price(101)),
        canonical_purchase(buyer(), listing(), version(4), price(101)),
        canonical_purchase(buyer(), listing(), version(3), price(102)),
        // 同字段的改价与购买操作码不同，摘要必不同。
        canonical_reprice_v2(buyer(), listing(), version(3), price(101)),
    ];
    for variant in variants {
        assert_ne!(request_hash(&base), request_hash(&variant));
    }
    // 反例：相同输入重算得到相同摘要，重试才会命中同一账本结果。
    assert_eq!(
        request_hash(&base),
        request_hash(&canonical_purchase(buyer(), listing(), version(3), price(101)))
    );
}

#[derive(Default)]
struct Recorder {
    purchases: Mutex<Vec<PurchaseCommand>>,
    reprices: Mutex<Vec<RepriceV2Command>>,
}

impl PurchaseStore for Recorder {
    fn purchase(&self, command: PurchaseCommand) -> ConsignmentFuture<'_, PurchaseOutcome> {
        Box::pin(async move {
            let receipt = PurchaseReceipt {
                command_id: command.command_id,
                listing_id: command.input.listing_id,
                listing_version: version(5),
                contract_id: ConsignmentContractId::from_uuid(Uuid::from_u128(9)),
                contract_version: 1,
                equipment_id: Uuid::from_u128(8),
                world: SettlementWorld::Main,
                price: command.input.expected_price,
                order_state: OrderState::AwaitingPayment,
                replayed: false,
            };
            self.purchases.lock().expect("锁").push(command);
            Ok(PurchaseOutcome::Purchased(receipt))
        })
    }

    fn replay_purchase(
        &self,
        _command: PurchaseCommand,
    ) -> ConsignmentFuture<'_, Option<PurchaseOutcome>> {
        Self::no_replay()
    }
}

impl Recorder {
    /// 只重放端口：本测试替身没有账本。
    fn no_replay() -> ConsignmentFuture<'static, Option<PurchaseOutcome>> {
        Box::pin(async { Ok(None) })
    }
}

impl RepriceV2Store for Recorder {
    fn reprice(&self, command: RepriceV2Command) -> ConsignmentFuture<'_, RepriceV2Outcome> {
        Box::pin(async move {
            let receipt = RepriceV2Receipt {
                command_id: command.command_id,
                listing_id: command.input.listing_id,
                listing_version: version(5),
                price: command.input.price,
                disposition: ListingPriceDisposition::Repriced,
                replayed: false,
            };
            self.reprices.lock().expect("锁").push(command);
            Ok(RepriceV2Outcome::Accepted(receipt))
        })
    }
}

fn client_command_id() -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0001))
}

fn purchase_input() -> PurchaseInput {
    PurchaseInput {
        listing_id: listing(),
        expected_version: version(3),
        expected_price: price(101),
    }
}

fn service() -> (Arc<Recorder>, PurchaseCommandService) {
    let recorder = Arc::new(Recorder::default());
    (recorder.clone(), PurchaseCommandService::new(recorder.clone(), recorder))
}

#[test]
fn purchase_hands_canonical_hash_to_store_and_ignores_request_id() {
    let (recorder, service) = service();
    for request_id in [None, Some("req-1".to_owned())] {
        run(service.purchase(buyer(), client_command_id(), purchase_input(), request_id))
            .expect("购买应转交存储");
    }
    let sent = recorder.purchases.lock().expect("锁");
    let expected = request_hash(&canonical_purchase(buyer(), listing(), version(3), price(101)));
    assert_eq!(sent.len(), 2);
    // request_id 只作审计关联：重试换请求号，摘要不变，账本才会按同请求重放。
    assert!(sent.iter().all(|command| command.request_hash == expected));
}

#[test]
fn invalid_client_inputs_never_reach_the_store() {
    let (recorder, service) = service();
    // 非 UUIDv4（后台派生的 v5、nil）的命令 ID 不得占用账本。
    for bad in [Uuid::nil(), Uuid::new_v5(&Uuid::NAMESPACE_URL, b"system")] {
        assert_eq!(
            run(service.purchase(buyer(), CommandId::from_uuid(bad), purchase_input(), None)),
            Err(ConsignmentError::InvalidCommandId)
        );
    }
    assert!(
        run(service.purchase(buyer(), client_command_id(), purchase_input(), Some(String::new())))
            .is_err()
    );
    let reprice = RepriceInput {
        seller_user_id: buyer(),
        listing_id: listing(),
        expected_version: version(3),
        price: price(120),
    };
    assert_eq!(
        run(service.reprice(CommandId::from_uuid(Uuid::nil()), reprice, None)),
        Err(ConsignmentError::InvalidCommandId)
    );
    assert!(recorder.purchases.lock().expect("锁").is_empty());
    assert!(recorder.reprices.lock().expect("锁").is_empty());
}

#[test]
fn funding_freeze_exists_only_for_ready_preflight() {
    let boundary_at = Timestamp::from_ymd_hms(2026, 11, 20, 0, 0, 0).expect("时刻");
    let ready = FundingPreflightFact::Ready {
        uid: "42".to_owned(),
        mode: FundingMode::RecordOnly,
        boundary_at,
        boundary_source_id: None,
        account_version: 3,
    };
    let PurchaseFunding::Ready(freeze) = purchase_funding(&ready) else {
        panic!("Ready 应映射为 Ready 并带冻结值");
    };
    assert_eq!(
        (freeze.uid.as_str(), freeze.mode, freeze.boundary_at),
        ("42", FundingAccountingMode::RecordOnly, boundary_at)
    );
    // 反例：任一不可用原因都不得冻结，也不得放行购买。
    for reason in [
        FundingUnavailable::AccountMissing,
        FundingUnavailable::OpeningBlocked,
        FundingUnavailable::EvidenceCapabilityMissing,
    ] {
        let blocked = FundingPreflightFact::Unavailable(reason);
        assert_eq!(purchase_funding(&blocked), PurchaseFunding::Unavailable);
    }
}
