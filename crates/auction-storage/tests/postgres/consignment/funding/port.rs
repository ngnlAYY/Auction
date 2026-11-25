//! 经 `ConsignmentFundingStore` 端口驱动资金写口：委托后金额规则（P = S + F）、世界与收款腿互校与命令
//! 幂等仍生效；现金腿与退款转换的订单阶段事件与资金事实同事务、重放不重复追加；本金争槽一成一冲突。

use std::time::Duration;

use auction_application::settlement::{
    CashPhase, ConsignmentFundingStore, ConvertRefund, FundingError,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::ListingQuote;
use auction_domain::consignment::funding::{FundingMode, PrincipalConclusion};
use sqlx::{Connection, PgPool};

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1, OBLIGATION_3};
use super::super::ledger136::fixture::PRICE;
use super::carrier::{
    CARRIER_DELIVERY, Dispatch, carrier_cash, force_carrier_obligation_into_main,
    seed_record_only_carrier,
};
use super::certify::Env;
use super::cod_fixture::{OPENING_ENTRY, Shape, certify, ledger, register};
use super::fee::{FEE_ENTRY, PAYMENT_SOURCE, fee_fact_count, funded_sql, quote, seed_sale};
use super::fixture::{UID, cash, entries_of, id, mutation, seed_refund_ready};
use crate::support::TestDatabase;
use crate::support::lock_waiters::count_blocked_by;

async fn order_events(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.realtime_events
         WHERE kind = 'consignment.order_stage_changed'",
    )
    .fetch_one(pool)
    .await
    .expect("读取订单阶段事件数")
}

/// 注入订单阶段事件写入失败：同事务的资金事实必须随之回滚。
async fn fail_order_events(pool: &PgPool) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "CREATE FUNCTION public.fake_fail_order_event() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'fake injected failure'; END $$;
         CREATE TRIGGER fake_fail_order_event
         BEFORE INSERT ON public.realtime_events
         FOR EACH ROW WHEN (NEW.kind = 'consignment.order_stage_changed')
         EXECUTE FUNCTION public.fake_fail_order_event();"
            .to_owned(),
    ))
    .execute(pool)
    .await
    .expect("注入事件写入失败");
}

async fn allow_order_events(pool: &PgPool) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "DROP TRIGGER fake_fail_order_event ON public.realtime_events;".to_owned(),
    ))
    .execute(pool)
    .await
    .expect("撤掉注入");
}

async fn refund_allocation_state(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT state FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = 'refund'",
    )
    .bind(UID)
    .fetch_one(pool)
    .await
    .expect("读取退款分配状态")
}

#[tokio::test]
async fn postgres_consignment_funding_port_certify_keeps_quote_and_leg_checks_and_replays() {
    let env = Env::new(Shape::STRICT).await;
    let pool = env.pool();
    let port: &dyn ConsignmentFundingStore = &env.store;

    // 反例：P ≠ S + F、世界与收款腿不符，经端口同样失败关闭，资金账本逐字不变。
    let before = ledger(pool).await;
    let mut forged_quote = certify(2, None);
    forged_quote.quote = ListingQuote { fee: 6, seller_amount: 96 };
    assert!(matches!(port.certify_cod(forged_quote).await, Err(FundingError::Invalid)));
    let mut crossed = certify(2, None);
    crossed.world = SettlementWorld::Main;
    assert!(matches!(port.certify_cod(crossed).await, Err(FundingError::Invalid)));
    assert_eq!(ledger(pool).await, before);
    assert_eq!(order_events(pool).await, 0);

    // 正例：认证一次带一条事件；同 ID 同请求回原回执、不再追加事件；同 ID 异请求冲突。
    let receipt = port.certify_cod(certify(2, None)).await.expect("经端口认证");
    assert!(!receipt.replayed);
    let again = port.certify_cod(certify(2, None)).await.expect("同命令重放");
    assert!(again.replayed);
    assert_eq!(again.receipt_source_id, receipt.receipt_source_id);
    let mut changed = certify(2, None);
    changed.mutation.request_digest = [9; 32];
    assert!(matches!(port.certify_cod(changed).await, Err(FundingError::Conflict)));
    assert_eq!(order_events(pool).await, 1, "重放与冲突不追加事件");
}

#[tokio::test]
async fn postgres_consignment_funding_port_opening_attribution_replays_original_receipt() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    super::cod_fixture::seed(pool, Shape::STRICT).await;
    let port: &dyn ConsignmentFundingStore = &store;

    let sealed = port
        .register_opening_attribution(register(1, &[(OPENING_ENTRY, 0, 60)]))
        .await
        .expect("经端口封存份额");
    assert_eq!((sealed.included, sealed.replayed), (60, false));
    let same = port
        .register_opening_attribution(register(1, &[(OPENING_ENTRY, 0, 60)]))
        .await
        .expect("同命令重放");
    assert_eq!(
        (same.attribution_id, same.included, same.replayed),
        (sealed.attribution_id, 60, true)
    );
    let mut changed = register(1, &[(OPENING_ENTRY, 0, 60)]);
    changed.mutation.request_digest = [9; 32];
    assert!(matches!(
        port.register_opening_attribution(changed).await,
        Err(FundingError::Conflict)
    ));
    // 份额登记不指向合同、不改变订单阶段，不追加事件。
    assert_eq!(order_events(pool).await, 0);
}

/// 现金腿：事件写入失败时迁移整笔回滚；成功的迁移恰一条事件，重放与无变化命令不追加。
#[tokio::test]
async fn postgres_consignment_funding_port_settle_cash_event_commits_with_the_move() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_refund_ready(pool).await;
    let port: &dyn ConsignmentFundingStore = &store;

    fail_order_events(pool).await;
    assert!(port.settle_cash(cash(1, CashPhase::Reserve, PRICE)).await.is_err());
    assert_eq!(refund_allocation_state(pool).await, "available", "预留随事件一并回滚");
    assert_eq!(entries_of(pool, UID, "reserve").await, (0, 0));
    allow_order_events(pool).await;

    let reserved = port.settle_cash(cash(1, CashPhase::Reserve, PRICE)).await.expect("预留");
    assert!(!reserved.replayed);
    assert_eq!(refund_allocation_state(pool).await, "reserved");
    assert_eq!(order_events(pool).await, 1);
    assert!(port.settle_cash(cash(1, CashPhase::Reserve, PRICE)).await.expect("重放").replayed);
    assert_eq!(order_events(pool).await, 1, "重放不追加事件");
    assert_eq!(entries_of(pool, UID, "reserve").await, (1, PRICE));
}

#[tokio::test]
async fn postgres_consignment_funding_port_settle_cash_rejects_world_and_leg_mismatch() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_record_only_carrier(pool, &Dispatch::GOOD).await;
    force_carrier_obligation_into_main(pool).await;
    let port: &dyn ConsignmentFundingStore = &store;

    // 载体义务落在 main 世界：经端口的载体本金腿迁移失败关闭，不发行、不预留、不追加事件。
    let result = port.settle_cash(carrier_cash(1, CashPhase::Reserve)).await;
    assert!(matches!(result, Err(FundingError::Invalid)));
    assert_eq!(entries_of(pool, UID, "issue").await, (0, 0));
    assert_eq!(order_events(pool).await, 0);
}

/// 退款转换：P ≠ S + F 在端口入口失败关闭；事件写入失败时转换整笔回滚；成功的转换恰一条事件，
/// 同 ID 同请求回原回执且不追加事件。
#[tokio::test]
async fn postgres_consignment_funding_port_convert_refund_checks_quote_and_event_is_atomic() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let (price, quote) = quote(500);
    seed_sale(pool, 5, 500).await;
    store
        .test_write_fee_earned(
            &funded_sql(5),
            id(CONTRACT),
            id(PAYMENT_SOURCE),
            id(FEE_ENTRY),
            quote,
        )
        .await
        .expect("funded");
    let port: &dyn ConsignmentFundingStore = &store;
    let command = ConvertRefund {
        mutation: mutation(20),
        contract_id: id(CONTRACT),
        refund_obligation_id: id(OBLIGATION_3),
        price,
        quote,
    };
    let transfers = entries_of(pool, UID, "transfer").await;

    let forged = ConvertRefund {
        quote: ListingQuote { fee: quote.fee, seller_amount: quote.seller_amount - 1 },
        ..command.clone()
    };
    assert!(matches!(port.convert_refund(forged).await, Err(FundingError::Invalid)));

    fail_order_events(pool).await;
    assert!(port.convert_refund(command.clone()).await.is_err());
    assert_eq!(fee_fact_count(pool, "reversed").await, 0, "转换随事件一并回滚");
    assert_eq!(entries_of(pool, UID, "transfer").await, transfers);
    allow_order_events(pool).await;

    let done = port.convert_refund(command.clone()).await.expect("经端口转换");
    assert!(!done.replayed);
    assert_eq!(done.conversion.to_refund, PRICE);
    assert_eq!(order_events(pool).await, 1);
    let again = port.convert_refund(command).await.expect("同命令重放");
    assert!(again.replayed);
    assert_eq!(again.conversion, done.conversion);
    assert_eq!(again.fee_reversal, done.fee_reversal);
    assert_eq!(order_events(pool).await, 1, "重放不追加事件");
    assert_eq!(fee_fact_count(pool, "reversed").await, 1);
}

/// 本金争槽：另一事务先写入同一槽位的付款本金并持有未提交，RTS 本金写口在唯一键上等待；
/// 先者提交后败方以 `Conflict`（而非可重试的存储失败）整笔回滚，槽位只留先者的结论。
#[tokio::test]
async fn postgres_consignment_funding_principal_slot_race_loser_gets_conflict() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    seed_record_only_carrier(pool, &Dispatch::GOOD).await;

    let mut winner = pool.acquire().await.expect("取得先者连接");
    let winner_pid: i32 =
        sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *winner).await.expect("pid");
    let mut tx = winner.begin().await.expect("先者开启事务");
    sqlx::query(
        "INSERT INTO public.consignment_principal_facts
            (id, contract_id, carrier_obligation_id, original_delivery_id, round, conclusion,
             mode, amount)
         VALUES (gen_random_uuid(), $1, $2, $3, 1, 'payment_principal', 'record_only', 1)",
    )
    .bind(id(CONTRACT))
    .bind(id(OBLIGATION_1))
    .bind(id(CARRIER_DELIVERY))
    .execute(&mut *tx)
    .await
    .expect("先者写入付款本金");

    let loser = tokio::spawn({
        let store = store.clone();
        async move {
            store
                .test_record_principal_fact(
                    id(CONTRACT),
                    id(OBLIGATION_1),
                    id(CARRIER_DELIVERY),
                    PrincipalConclusion::RtsPrincipal,
                    FundingMode::RecordOnly,
                )
                .await
        }
    });
    let mut waited = 0;
    while count_blocked_by(pool, winner_pid).await.expect("读取锁等待") == 0 {
        assert!(waited < 200, "败方没有在唯一键上等待先者");
        tokio::time::sleep(Duration::from_millis(25)).await;
        waited += 1;
    }
    tx.commit().await.expect("先者提交");

    let result = loser.await.expect("败方任务");
    assert!(matches!(result, Err(FundingError::Conflict)), "败方必须是冲突：{result:?}");
    let facts: Vec<String> =
        sqlx::query_scalar("SELECT conclusion FROM public.consignment_principal_facts")
            .fetch_all(pool)
            .await
            .expect("读取本金结论");
    assert_eq!(facts, vec!["payment_principal".to_owned()]);
}
