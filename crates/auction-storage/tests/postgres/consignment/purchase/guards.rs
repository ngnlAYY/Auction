//! 购买的写前闸门：买家待执行处罚阻断、严格现金必须有开户边界来源、资金预检不在持锁事务内取连接。
//!
//! 保护不变量 2（锁内读事实、持锁期间不等待池）、6（缺事实一律失败关闭，不静默放行）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use auction_application::consignment::{PurchaseCommand, PurchaseOutcome, PurchaseStore};
use auction_domain::consignment::purchase::PurchaseRejection;
use auction_storage::TestAfterLock;

use super::super::refusal::fixture::{buyer as refusal_buyer, command as refusal_command};
use super::fixture::{
    BUYER, Fixture, Funding, PRICE, USER_3, command_id, count_advisory_locks, purchase,
};

/// 只读：一条回执的 (状态码, 稳定码)。
async fn receipt_of(fixture: &Fixture, seed: u128) -> (i16, String) {
    sqlx::query_as(
        "SELECT result_status, result_code FROM public.consignment_commands WHERE command_id = $1",
    )
    .bind(command_id(seed).as_uuid())
    .fetch_one(fixture.pool())
    .await
    .expect("回执")
}

/// 被测挂单的购买命令（夹具命令默认指向存量里已购的那件）。
fn buy(fixture: &Fixture, buyer: &str, seed: u128, hash: u8) -> PurchaseCommand {
    let mut command = purchase(buyer, seed, hash);
    command.input.listing_id = fixture.listing;
    command
}

/// 成交事实的行数快照：合同、Sale、义务、履约行、首轮。
async fn sale_rows(fixture: &Fixture) -> Vec<i64> {
    let mut rows = Vec::new();
    for table in [
        "consignment_contracts",
        "consignment_sales",
        "settlement_obligations",
        "settlement_obligation_fulfillments",
        "consignment_dispatch_rounds",
    ] {
        rows.push(fixture.count(table).await);
    }
    rows
}

/// 没有新增任何成交事实：行数与`before`相同，被测挂单仍 available、位置仍 in_custody。
async fn assert_nothing_sold(fixture: &Fixture, before: &[i64]) {
    assert_eq!(sale_rows(fixture).await, before, "不应新增成交事实");
    assert_eq!(fixture.listing().await, ("available".to_owned(), 5, PRICE));
    assert_eq!(fixture.position_state().await, "in_custody");
}

/// 买家有 pending 处罚（拒收提交链造出的真实存量）：购买得确定拒绝 403 participant_ineligible，
/// 无任何新增成交事实，重放仍是同一拒绝；没有处罚的买家照常成交。
#[tokio::test]
async fn postgres_consignment_purchase_pending_sanction_blocks_the_buyer() {
    let fixture = Fixture::beside_pending_sanction().await;
    let before = sale_rows(&fixture).await;
    let blocked = fixture.repository.purchase(buy(&fixture, BUYER, 11, 0x11)).await.expect("拒绝");
    assert!(
        matches!(blocked, PurchaseOutcome::Rejected(PurchaseRejection::ParticipantIneligible)),
        "{blocked:?}"
    );
    assert_eq!(receipt_of(&fixture, 11).await, (403, "participant_ineligible".to_owned()));
    assert_nothing_sold(&fixture, &before).await;
    let replay = fixture.repository.purchase(buy(&fixture, BUYER, 11, 0x11)).await.expect("重放");
    assert!(
        matches!(replay, PurchaseOutcome::Rejected(PurchaseRejection::ParticipantIneligible)),
        "{replay:?}"
    );
    assert_nothing_sold(&fixture, &before).await;

    // 阻断只看买家：没有处罚的 USER_3 照常成交。
    let other =
        fixture.repository.purchase(buy(&fixture, USER_3, 12, 0x12)).await.expect("他人购买");
    assert!(matches!(other, PurchaseOutcome::Purchased(_)), "{other:?}");
    assert_eq!(fixture.listing().await, ("sale_pending".to_owned(), 6, PRICE));
}

/// 解禁（关闭旧周期、处罚转 cycle_closed）之后，同一买家可以成交。
#[tokio::test]
async fn postgres_consignment_purchase_unblocked_buyer_can_buy() {
    let fixture = Fixture::beside_pending_sanction().await;
    let before = sale_rows(&fixture).await;
    let blocked = fixture.repository.purchase(buy(&fixture, BUYER, 11, 0x11)).await.expect("拒绝");
    assert!(
        matches!(blocked, PurchaseOutcome::Rejected(PurchaseRejection::ParticipantIneligible)),
        "{blocked:?}"
    );
    assert_nothing_sold(&fixture, &before).await;

    fixture
        .runtime
        .test_refusal_close_cycle(refusal_command(100), refusal_buyer(), None)
        .await
        .expect("解禁");
    let bought = fixture.repository.purchase(buy(&fixture, BUYER, 12, 0x12)).await.expect("成交");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    assert_eq!(fixture.listing().await, ("sale_pending".to_owned(), 6, PRICE));
    assert_eq!(fixture.count("consignment_contracts").await, before[0] + 1);
}

/// 严格现金没有开户边界来源：失败关闭为确定的 503 funding_unavailable，不成交；
/// record_only 没有现金 opening 时允许空来源，照常成交并写入空来源。
#[tokio::test]
async fn postgres_consignment_purchase_strict_cash_needs_a_boundary_source() {
    let fixture = Fixture::available().await;
    fixture.preflight.set(Funding::StrictCashNoBoundary);
    let rejected = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("拒绝");
    assert!(
        matches!(rejected, PurchaseOutcome::Rejected(PurchaseRejection::FundingUnavailable)),
        "{rejected:?}"
    );
    assert_eq!(receipt_of(&fixture, 1).await, (503, "funding_unavailable".to_owned()));
    assert_nothing_sold(&fixture, &[0, 0, 0, 0, 0]).await;

    fixture.preflight.set(Funding::RecordOnlyNoBoundary);
    let bought = fixture.repository.purchase(purchase(BUYER, 2, 0x12)).await.expect("成交");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    let (mode, source): (String, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT funding_mode, funding_boundary_source_id FROM public.consignment_sales",
    )
    .fetch_one(fixture.pool())
    .await
    .expect("Sale 资金冻结");
    assert_eq!((mode.as_str(), source), ("record_only", None));
}

/// 资金预检经连接池另取连接，所以必须在开事务、取会话锁之前读：预检被调用时旁路看不到任何会话咨询锁；
/// 同一探针在取锁之后确实数得到锁（排除探针本身失效）。
#[tokio::test]
async fn postgres_consignment_purchase_preflight_runs_before_any_lock_is_held() {
    let fixture = Fixture::available().await;
    let bought = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("成交");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    assert_eq!(fixture.preflight.advisory_locks_seen(), vec![0], "预检不得在持锁事务内调用");

    let fixture = Fixture::available().await;
    let held = Arc::new(AtomicI64::new(-1));
    let hook: Box<TestAfterLock> = {
        let (probe, held) = (fixture.pool().clone(), held.clone());
        Box::new(move || -> Pin<Box<dyn Future<Output = ()> + Send>> {
            let (probe, held) = (probe.clone(), held.clone());
            Box::pin(
                async move { held.store(count_advisory_locks(&probe).await, Ordering::SeqCst) },
            )
        })
    };
    let bought = fixture
        .repository
        .test_purchase_hooked(purchase(BUYER, 1, 0x11), Some(&*hook))
        .await
        .expect("成交");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    assert!(held.load(Ordering::SeqCst) > 0, "取锁之后探针应看到会话咨询锁");
    assert_eq!(fixture.preflight.advisory_locks_seen(), vec![0]);
}
