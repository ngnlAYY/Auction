//! 消费：证据不足持续核对且零新增 POST；精确证明、消费与资金回补同事务且只一次。

use std::sync::Arc;

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection, WriteStart};
use auction_application::settlement::{
    FundingCommitOutcome, FundingError, FundingReconciliationReader,
};
use auction_storage::DatabaseStore;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{DELIVERY, POSITION, succeeds};
use super::super::funding::fixture::{
    UID, entries_of, ledger_counts, reconcile_with, try_reconcile,
};
use super::super::ledger136::fixture::{PAY_DELIVERY, PRICE};
use super::super::ledger136::purchase::buyer_claimed_original_from;
use super::fixture::{
    AMOUNT, Claim, PAYOUT, Rts, SIGNED_VALIDATOR, UNSIGNED_VALIDATOR, consume_input, seed_consumed,
    seed_original_started, seed_payout_started, seed_started_unproven, service, start,
};
use crate::support::TestDatabase;

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

/// (证明数, 消费数, 原资金回补数, 未解除阻断数)。
async fn effects(pool: &PgPool) -> (i64, i64, i64, i64) {
    (
        count(
            pool,
            "SELECT count(*) FROM public.mail_disposition_proofs
             WHERE work_id IN (SELECT take_work_id FROM public.consignment_return_intakes)",
        )
        .await,
        count(pool, "SELECT count(*) FROM public.consignment_return_consumptions").await,
        count(pool, "SELECT count(*) FROM public.consignment_funding_returns").await,
        count(
            pool,
            "SELECT count(*) FROM public.consignment_cash_return_blocks WHERE state = 'open'",
        )
        .await,
    )
}

fn rejection(error: &ConsignmentError) -> Option<DepositBusinessRejection> {
    match error {
        ConsignmentError::Deposit(rejection) => Some(*rejection),
        _ => None,
    }
}

async fn setup(database: &TestDatabase) -> (Arc<DatabaseStore>, DatabaseStore) {
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    (store, owner)
}

/// 已 Take 但证据不足（验证器未签收，生产的现状）：持续核对，零新增 POST，阻断与责任保留。
#[tokio::test]
async fn postgres_consignment_returns_unproven_take_keeps_reconcile_without_new_post() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_started_unproven(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);

    for _ in 0..2 {
        let error = service
            .consume(consume_input(&receipt, Rts { n: 1 }, claim, UNSIGNED_VALIDATOR, 0xa7))
            .await
            .expect_err("未签收验证器不得消费");
        assert_eq!(rejection(&error), Some(DepositBusinessRejection::EvidenceUnproven));
    }
    assert_eq!(effects(pool).await, (0, 0, 0, 1), "零证明、零消费、零回补，阻断仍在");
    assert_eq!(entries_of(pool, UID, "return").await, (0, 0));

    // 新 fence 重新领取也只能核对：已起写的工作得不到第二个许可。
    let next = Claim::new(22, 2, true);
    super::fixture::claim_take(
        pool,
        &receipt,
        super::fixture::take_delivery(pool, &receipt).await,
        next,
    )
    .await;
    let again = start(&service, pool, &receipt, next).await.expect("只核对");
    assert!(!matches!(again, WriteStart::FirstCommitted { .. }));
}

/// 精确证明：证明、消费与资金回补同事务且只一次；重放无新增，异证明冲突，错引用拒绝。
#[tokio::test]
async fn postgres_consignment_returns_consume_writes_proof_consumption_and_cash_return_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_started_unproven(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);

    // 反例：命令引用的 fence 不是原起写 fence，是矛盾，零写入。
    let mut wrong = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    wrong.fence = 9;
    let error = service.consume(wrong).await.expect_err("错误引用不得消费");
    assert_eq!(rejection(&error), Some(DepositBusinessRejection::EvidenceConflict));
    assert_eq!(effects(pool).await, (0, 0, 0, 1));

    let input = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    let consumed = service.consume(input.clone()).await.expect("消费");
    assert!(!consumed.replayed);
    assert_eq!(effects(pool).await, (1, 1, 1, 0), "证明、消费、回补各一，阻断解除");
    assert_eq!(entries_of(pool, UID, "return").await, (1, AMOUNT), "恰一笔全额 return");

    let replay = service.consume(input).await.expect("重放");
    assert!(replay.replayed);
    assert_eq!(replay.proof_id, consumed.proof_id);
    assert_eq!(effects(pool).await, (1, 1, 1, 0), "重放无新增");
    assert_eq!(entries_of(pool, UID, "return").await, (1, AMOUNT));

    // 同 intake 换一份证明：同命令 ID 异请求冲突，不产生第二次恢复。
    let other = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xb8);
    let conflict = service.consume(other).await.expect_err("异请求必须冲突");
    assert!(matches!(conflict, ConsignmentError::CommandIdConflict));
    assert_eq!(effects(pool).await, (1, 1, 1, 0));
}

/// 回补失败则证明与消费一并回滚（不存在“有证明无消费”或“有消费无回补”的半态）。
#[tokio::test]
async fn postgres_consignment_returns_consume_rolls_back_proof_and_consumption_when_funding_fails()
{
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_started_unproven(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    succeeds(
        pool,
        "CREATE FUNCTION public.fixture_fail_return() RETURNS trigger LANGUAGE plpgsql AS
         $f$ BEGIN RAISE EXCEPTION '夹具：回补写入失败'; END $f$;
         CREATE TRIGGER fixture_fail_return BEFORE INSERT ON public.consignment_funding_returns
         FOR EACH ROW EXECUTE FUNCTION public.fixture_fail_return();",
    )
    .await;
    let input = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    assert!(service.consume(input.clone()).await.is_err());
    assert_eq!(effects(pool).await, (0, 0, 0, 1), "证明、消费、回补全部回滚，阻断仍在");
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'consume_return'"
        )
        .await,
        0
    );

    succeeds(pool, "DROP TRIGGER fixture_fail_return ON public.consignment_funding_returns;").await;
    service.consume(input).await.expect("同一命令重试成功");
    assert_eq!(effects(pool).await, (1, 1, 1, 0));
}

/// 交给第 7 期的存量构造器：已起写未证明、已消费两个状态可被复用。
#[tokio::test]
async fn postgres_consignment_returns_stock_constructors_build_started_and_consumed_states() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    seed_started_unproven(pool, &store, Rts { n: 1 }).await;
    assert_eq!(effects(pool).await, (0, 0, 0, 1), "已起写、未证明：阻断在，没有消费");

    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    seed_consumed(pool, &store, Rts { n: 1 }).await;
    assert_eq!(effects(pool).await, (1, 1, 1, 0), "已消费：证明、消费、回补各一，阻断解除");
}

/// (position 状态, 版本, 该版本之后的代管事件数)。
async fn position_of(pool: &PgPool) -> (String, i64, i64) {
    let (state, version): (String, i64) = sqlx::query_as(
        "SELECT state, version FROM public.consignment_custody_positions WHERE id = $1::uuid",
    )
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("读取 position");
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_custody_events
         WHERE position_id = $1::uuid AND version > 4",
    )
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("读取事件数");
    (state, version, events)
}

/// 原件腿：消费后 position 由 in_transit 回到 committed，代管事件恰增一条；证明、消费与回位同事务。
#[tokio::test]
async fn postgres_consignment_returns_consume_original_leg_restores_position_with_one_event() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_original_started(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    assert_eq!(position_of(pool).await, ("in_transit".to_owned(), 4, 0), "消费前仍在途");

    let input = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    let consumed = service.consume(input.clone()).await.expect("消费");
    assert!(!consumed.replayed);
    assert_eq!(
        position_of(pool).await,
        ("committed".to_owned(), 5, 1),
        "回位恰推进一版并追加一条事件"
    );
    let (kind, from, to, listing_version): (String, String, String, i64) = sqlx::query_as(
        "SELECT event_kind, from_state, to_state, listing_version
         FROM public.consignment_custody_events WHERE position_id = $1::uuid AND version = 5",
    )
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("读取回位事件");
    assert_eq!(
        (kind.as_str(), from.as_str(), to.as_str()),
        ("committed", "in_transit", "committed")
    );
    assert_eq!(listing_version, 6);
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_return_consumptions WHERE leg = 'original'"
        )
        .await,
        1
    );
    // 原件腿不动资金：没有回补，也没有现金阻断。
    assert_eq!(effects(pool).await, (1, 1, 0, 0));

    // 重放不再改 position，也不再追加事件。
    assert!(service.consume(input).await.expect("重放").replayed);
    assert_eq!(position_of(pool).await, ("committed".to_owned(), 5, 1));
}

/// 回位事件写入失败则证明、消费与回位一并回滚；同一命令重试成功。
#[tokio::test]
async fn postgres_consignment_returns_consume_original_leg_rolls_back_when_the_custody_event_fails()
{
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_original_started(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    succeeds(
        pool,
        "CREATE FUNCTION public.fixture_fail_event() RETURNS trigger LANGUAGE plpgsql AS
         $f$ BEGIN RAISE EXCEPTION '夹具：代管事件写入失败'; END $f$;
         CREATE TRIGGER fixture_fail_event BEFORE INSERT ON public.consignment_custody_events
         FOR EACH ROW EXECUTE FUNCTION public.fixture_fail_event();",
    )
    .await;
    let input = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    assert!(service.consume(input.clone()).await.is_err());
    assert_eq!(effects(pool).await, (0, 0, 0, 0), "证明与消费回滚");
    assert_eq!(position_of(pool).await, ("in_transit".to_owned(), 4, 0), "position 不动");

    succeeds(pool, "DROP TRIGGER fixture_fail_event ON public.consignment_custody_events;").await;
    service.consume(input).await.expect("同一命令重试成功");
    assert_eq!(position_of(pool).await, ("committed".to_owned(), 5, 1));
}

/// 不变量 6：原件 position 不在 in_transit（如核对中）时失败关闭，整笔回滚、不占命令 ID。
#[tokio::test]
async fn postgres_consignment_returns_consume_original_leg_fails_closed_on_illegal_position_state()
{
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_original_started(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    // 夹具直接把 position 置为核对中：真实的 in_transit -> reconcile 需要核对证明，与本用例无关，
    // 这里在同一事务内停用该表的用户触发器，改完立即恢复。
    succeeds(
        pool,
        &format!(
            "ALTER TABLE public.consignment_custody_positions DISABLE TRIGGER USER;
             UPDATE public.consignment_custody_positions
             SET state = 'reconcile', version = 5, updated_at = statement_timestamp()
             WHERE id = '{POSITION}';
             ALTER TABLE public.consignment_custody_positions ENABLE TRIGGER USER;"
        ),
    )
    .await;
    let input = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    assert!(service.consume(input).await.is_err(), "非法前态不得消费");
    assert_eq!(effects(pool).await, (0, 0, 0, 0), "证明与消费回滚");
    assert_eq!(position_of(pool).await, ("reconcile".to_owned(), 5, 0), "position 保持核对中");
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'consume_return'"
        )
        .await,
        0
    );
}

/// 卖款分配的 (状态, 绑定的 delivery, 金额)。
async fn seller_allocation(pool: &PgPool) -> (String, Option<Uuid>, i64) {
    sqlx::query_as(
        "SELECT state, delivery_id, amount FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'seller'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取卖款分配")
}

/// 卖款腿 S（D9 §11.1“现金退件与本 UID opening”）：Take 起写之后、资金消费之前，本 UID 的 opening
/// 最终写被拒，零 source、零 issue、边界不动；精确 Take 消费恰一笔全额 return 回原 seller 用途并解除阻断，
/// 重放无第二次恢复；随后在可合法对账的例中重做 opening，对这笔金额新增发行 0。
///
/// 不变量 2、4、6：已外发的卖款结果只能由精确 Take 的回补解除，opening 不能把在途退件当成余额重新发行。
#[tokio::test]
async fn postgres_consignment_returns_cash_payout_blocks_opening_until_consumed_then_reopening_issues_nothing()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_payout_started(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    assert_eq!(effects(pool).await, (0, 0, 0, 1), "起写同事务建立卖款腿阻断");
    assert_eq!(seller_allocation(pool).await.0, "spent");

    // 反例：消费前的 opening 被拒，账本边界不动。
    let before = ledger_counts(pool, UID).await;
    let issued = entries_of(pool, UID, "issue").await;
    let rejected = try_reconcile(&store, pool, UID).await;
    assert!(
        matches!(rejected, Ok(FundingCommitOutcome::RolledBack(FundingError::Unproven))),
        "{rejected:?}"
    );
    assert_eq!(ledger_counts(pool, UID).await, before, "被拒的 opening 不写任何来源或分录");
    assert_eq!(entries_of(pool, UID, "issue").await, issued);

    // 正例：精确 Take 消费恰一笔全额 return，原 seller 用途恢复，阻断同事务解除；重放无新增。
    let input = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    assert!(!service.consume(input.clone()).await.expect("消费").replayed);
    assert_eq!(effects(pool).await, (1, 1, 1, 0), "证明、消费、回补各一，阻断解除");
    assert_eq!(entries_of(pool, UID, "return").await, (1, PAYOUT), "恰一笔全额 return");
    assert_eq!(seller_allocation(pool).await, ("available".to_owned(), None, PAYOUT));
    assert!(service.consume(input).await.expect("重放").replayed);
    assert_eq!(entries_of(pool, UID, "return").await, (1, PAYOUT), "重放无第二次恢复");

    // 合法重做 opening：取真实预览的摘要与版本提交；已回补的 S 不再发行。
    // 共用夹具的存入 Take 与原件首派的投递外壳省略了生产载荷里的 credits / cod（生产的 Take 工作与首派都写
    // 这两项），预览会把它们判为资产无界；这里按各自的冻结身份补成生产形状，不放宽任何约束。
    succeeds(
        pool,
        &format!(
            "UPDATE public.deliveries SET payload = payload || '{{\"credits\":0,\"cod\":0}}'::jsonb
             WHERE id = '{DELIVERY}';
             UPDATE public.deliveries SET payload = payload || '{{\"credits\":0,\"cod\":{PRICE}}}'::jsonb
             WHERE id = '{PAY_DELIVERY}';"
        ),
    )
    .await;
    let preview = store.preview_funding(UID.to_owned()).await.expect("资金预览");
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
    let sources = ledger_counts(pool, UID).await.0;
    let outcome = reconcile_with(&store, pool, UID, preview.account_version, preview.digest).await;
    match outcome {
        Ok(FundingCommitOutcome::Committed(result)) => {
            assert_eq!((result.source_id, result.allocated_credits), (None, 0));
        }
        other => panic!("合法对账应提交：{other:?}"),
    }
    assert_eq!(entries_of(pool, UID, "issue").await, issued, "对这笔金额新增发行 0");
    assert_eq!(ledger_counts(pool, UID).await.0, sources, "不新增 opening 来源");
    assert_eq!(seller_allocation(pool).await, ("available".to_owned(), None, PAYOUT));
}

/// 原件腿的成对证据（D9 §11.1“购买与正证”）：买家已确领原件（确领证明与 handed_over 已提交）之后，
/// 同一轮的 RTS 即使有已签收的精确 Take 证明也不能消费——原物领取与原物 RTS 互斥，整笔回滚、
/// 零证明、零消费、position 不动、不占命令 ID。正例见
/// `postgres_consignment_returns_consume_original_leg_restores_position_with_one_event`。
#[tokio::test]
async fn postgres_consignment_returns_original_rts_is_not_consumed_after_the_buyer_claimed_it() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_original_started(pool, &store, Rts { n: 1 }).await;
    succeeds(pool, &buyer_claimed_original_from("in_transit", 5)).await;
    let service = service(&store);

    let input = consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7);
    assert!(service.consume(input).await.is_err(), "确领之后的 RTS 不得消费");
    assert_eq!(effects(pool).await, (0, 0, 0, 0), "零证明、零消费");
    assert_eq!(position_of(pool).await, ("handed_over".to_owned(), 5, 1), "position 保持已交付");
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'consume_return'"
        )
        .await,
        0
    );
}
