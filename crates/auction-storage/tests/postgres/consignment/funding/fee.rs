//! 手续费与退款转换：F>0 恰一条 earned、首次退款恰一条 reversed；F=0 全无；补偿池零增量。

use auction_application::settlement::{
    CashLeg, CashPhase, ConvertRefund, FundingError, SettleCash,
};
use auction_domain::consignment::{FeeBps, ListingPrice, ListingQuote, quote_v1};
use sqlx::PgPool;

use super::super::contracts::fixture::{
    CONTRACT, OBLIGATION_1, OBLIGATION_2, OBLIGATION_3, seed_main_custody,
};
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::{
    PAY_DELIVERY, PAY_MID, PRICE, PURCHASE_COMMAND, PURCHASED_AT, available_sql,
    original_dispatch_sql, purchase_command, purchase_listing, purchase_position, sale_graph,
    send_sql,
};
use super::fixture::{UID, entries_of, id, mutation};
use crate::support::TestDatabase;

const RECEIPT_KEY: &str = "c6d00000-0000-4000-8000-000000000001";
pub(super) const PAYMENT_SOURCE: &str = "c6d00000-0000-4000-8000-000000000002";
const TRANSIT: &str = "c6d00000-0000-4000-8000-000000000003";
const SELLER: &str = "c6d00000-0000-4000-8000-000000000004";
const FEE: &str = "c6d00000-0000-4000-8000-000000000005";
pub(super) const FEE_ENTRY: &str = "c6d10000-0000-4000-8000-000000000003";

/// 一份 funded 的 main 成交：A=0，全部为新发行 P，转出 S（与 F>0 时的 F）；含 funded 投影。
pub(super) fn funded_sql(fee: i64) -> String {
    let seller = PRICE - fee;
    let mut sql = format!(
        "INSERT INTO public.settlement_funding_sources (id, uid, kind, source_key, amount, proof_digest)
         VALUES ('{PAYMENT_SOURCE}', '{UID}', 'collected_payment', '{PAYMENT_SOURCE}', {PRICE},
                 decode(repeat('02', 32), 'hex'));
         INSERT INTO public.settlement_funding_receipts
            (source_id, uid, payment_obligation_id, delivery_id, external_mail_id, evidence)
         VALUES ('{PAYMENT_SOURCE}', '{UID}', '{OBLIGATION_1}', '{PAY_DELIVERY}', '{PAY_MID}',
                 '{{}}'::jsonb);
         INSERT INTO public.consignment_cod_receipts
            (source_id, uid, contract_id, payment_obligation_id, delivery_id, world_id, sent_mid,
             sent_fence, cod_leg, remote_payment_ref, receipt_key_id, gross, opening_included,
             new_issue)
         VALUES ('{PAYMENT_SOURCE}', '{UID}', '{CONTRACT}', '{OBLIGATION_1}', '{PAY_DELIVERY}',
                 'main', '{PAY_MID}', 1, 'original', NULL, '{RECEIPT_KEY}', {PRICE}, 0, {PRICE});
         INSERT INTO public.settlement_funding_allocations
            (id, uid, purpose, obligation_id, consignment_contract_id, amount)
         VALUES ('{TRANSIT}', '{UID}', 'consignment_transit', NULL, '{CONTRACT}', 0),
                ('{SELLER}', '{UID}', 'seller', '{OBLIGATION_2}', '{CONTRACT}', {seller});
         INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, debit_id, credit_id, source_id, amount, kind, proof_digest)
         VALUES ('c6d10000-0000-4000-8000-000000000001', '{PAYMENT_SOURCE}', '{UID}', NULL,
                 '{TRANSIT}', '{PAYMENT_SOURCE}', {PRICE}, 'issue', decode(repeat('03', 32), 'hex')),
                ('c6d10000-0000-4000-8000-000000000002', '{PAYMENT_SOURCE}', '{UID}', '{TRANSIT}',
                 '{SELLER}', NULL, {seller}, 'transfer', decode(repeat('03', 32), 'hex'));"
    );
    if fee > 0 {
        sql += &format!(
            "INSERT INTO public.settlement_funding_allocations
                (id, uid, purpose, obligation_id, consignment_contract_id, amount)
             VALUES ('{FEE}', '{UID}', 'fee', NULL, '{CONTRACT}', {fee});
             INSERT INTO public.settlement_funding_entries
                (id, command_id, uid, debit_id, credit_id, source_id, amount, kind, proof_digest)
             VALUES ('{FEE_ENTRY}', '{PAYMENT_SOURCE}', '{UID}', '{TRANSIT}', '{FEE}', NULL,
                     {fee}, 'transfer', decode(repeat('03', 32), 'hex'));"
        );
    }
    sql + &format!(
        "INSERT INTO public.consignment_funded_sales
            (contract_id, uid, payment_obligation_id, receipt_source_id)
         VALUES ('{CONTRACT}', '{UID}', '{OBLIGATION_1}', '{PAYMENT_SOURCE}');"
    )
}

/// 已购成交（P=101；fee_bps 由 `bps` 决定）加原件腿首派。
pub(super) async fn seed_sale(pool: &PgPool, fee: i64, bps: i64) {
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;
    let mut graph = sale_graph();
    graph.fee = fee;
    graph.seller_amount = PRICE - fee;
    graph.payout_amount = PRICE - fee;
    let graph_sql = graph.sql(PURCHASE_COMMAND).replace("1, 500, 7,", &format!("1, {bps}, 7,"));
    succeeds(pool, &(purchase_command() + &graph_sql + &purchase_position() + &purchase_listing()))
        .await;
    succeeds(pool, &original_dispatch_sql()).await;
}

pub(super) fn quote(bps: i64) -> (ListingPrice, ListingQuote) {
    let price = ListingPrice::new(PRICE).expect("价格");
    (price, quote_v1(price, FeeBps::new(bps).expect("费率")))
}

pub(super) async fn fee_fact_count(pool: &PgPool, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_fee_facts WHERE kind = $1")
        .bind(kind)
        .fetch_one(pool)
        .await
        .expect("读取手续费事实")
}

/// 补偿池三项：余额与版本、流水行数、冲回行数。
async fn compensation_snapshot(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT COALESCE(sum(balance), 0)::bigint FROM public.compensation_pool),
                (SELECT COALESCE(sum(state_version), 0)::bigint FROM public.compensation_pool),
                (SELECT count(*) FROM public.compensation_ledger),
                (SELECT count(*) FROM public.compensation_pool_reversals)",
    )
    .fetch_one(pool)
    .await
    .expect("读取补偿池")
}

async fn allocation_amount(pool: &PgPool, purpose: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT amount FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = $2 AND consignment_contract_id = $3",
    )
    .bind(UID)
    .bind(purpose)
    .bind(id(CONTRACT))
    .fetch_one(pool)
    .await
    .expect("读取分配")
}

#[tokio::test]
async fn postgres_consignment_funding_fee_earned_once_and_reversed_once_on_first_refund() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let (price, quote) = quote(500);
    assert_eq!((quote.fee, quote.seller_amount), (5, 96));
    seed_sale(pool, 5, 500).await;
    let (contract, source, fee_entry) = (id(CONTRACT), id(PAYMENT_SOURCE), id(FEE_ENTRY));

    // 反例：报价不是成交冻结的那一份，不写 earned（funded 投影随之回滚）。
    let (_, other_quote) = self::quote(100);
    let forged =
        store.test_write_fee_earned(&funded_sql(5), contract, source, fee_entry, other_quote).await;
    assert!(matches!(forged, Err(FundingError::Conflict)));
    assert_eq!(fee_fact_count(pool, "earned").await, 0);

    // 正例：F>0 恰一条 earned；重放不新增。
    let earned = store
        .test_write_fee_earned(&funded_sql(5), contract, source, fee_entry, quote)
        .await
        .expect("记录 earned");
    assert!(earned.is_some());
    let again =
        store.test_write_fee_earned("", contract, source, fee_entry, quote).await.expect("重放");
    assert_eq!(again, earned);
    assert_eq!(fee_fact_count(pool, "earned").await, 1);

    // 正例：首次退款转换恰一条 reversed，S 与 F 合并为买家退款 P；补偿池三项零增量。
    let pool_before = compensation_snapshot(pool).await;
    let command = ConvertRefund {
        mutation: mutation(20),
        contract_id: contract,
        refund_obligation_id: id(OBLIGATION_3),
        price,
        quote,
    };
    let done = store.test_convert_refund(command.clone()).await.expect("退款转换");
    assert!(!done.replayed);
    assert_eq!(
        (done.conversion.from_seller, done.conversion.from_fee, done.conversion.to_refund),
        (96, 5, 101)
    );
    assert_eq!(done.fee_reversal.map(|reversal| reversal.amount), Some(5));
    assert_eq!(fee_fact_count(pool, "reversed").await, 1);
    assert_eq!(
        (
            allocation_amount(pool, "refund").await,
            allocation_amount(pool, "seller").await,
            allocation_amount(pool, "fee").await
        ),
        (101, 0, 0)
    );
    assert_eq!(compensation_snapshot(pool).await, pool_before, "寄售对补偿池零增量");

    // 重放同命令无新增；换命令再次转换为冲突（reversed 至多一次、退款至多一笔）。
    let transfers = entries_of(pool, UID, "transfer").await;
    assert!(store.test_convert_refund(command.clone()).await.expect("重放").replayed);
    let second = ConvertRefund { mutation: mutation(21), ..command };
    assert!(matches!(store.test_convert_refund(second).await, Err(FundingError::Conflict)));
    assert_eq!(fee_fact_count(pool, "reversed").await, 1);
    assert_eq!(entries_of(pool, UID, "transfer").await, transfers);
}

#[tokio::test]
async fn postgres_consignment_funding_zero_fee_has_no_fee_rows_and_refunds_from_seller_only() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let (price, quote) = quote(0);
    assert_eq!((quote.fee, quote.seller_amount), (0, 101));
    seed_sale(pool, 0, 0).await;
    let earned = store
        .test_write_fee_earned(
            &funded_sql(0),
            id(CONTRACT),
            id(PAYMENT_SOURCE),
            id(FEE_ENTRY),
            quote,
        )
        .await
        .expect("F=0 不记 earned");
    assert_eq!(earned, None);
    assert_eq!(fee_fact_count(pool, "earned").await, 0);

    let done = store
        .test_convert_refund(ConvertRefund {
            mutation: mutation(30),
            contract_id: id(CONTRACT),
            refund_obligation_id: id(OBLIGATION_3),
            price,
            quote,
        })
        .await
        .expect("退款转换");
    // F=0：没有 reversed、没有 fee 分配与 fee→refund 分录，只有 seller→refund 一笔 P。
    assert_eq!(done.fee_reversal, None);
    assert_eq!(fee_fact_count(pool, "reversed").await, 0);
    assert_eq!(
        entries_of(pool, UID, "transfer").await,
        (2, 2 * PRICE),
        "付款分账一笔、退款转换一笔"
    );
    assert_eq!(allocation_amount(pool, "refund").await, PRICE);
    let fee_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_funding_allocations WHERE purpose = 'fee'",
    )
    .fetch_one(pool)
    .await
    .expect("读取 fee 分配");
    assert_eq!(fee_rows, 0);
}

const SELLER_DELIVERY: &str = "c6e10000-0000-4000-8000-000000000001";
const SELLER_ROUND: &str = "c6e20000-0000-4000-8000-000000000001";
const SELLER_ATTEMPT: &str = "c6e30000-0000-4000-8000-000000000001";

/// 卖款腿首派：Send、Sent MID 绑定与冻结金额等于 S 的轮次（只用于“卖款可能已发”的反例）。
fn seller_dispatch_sql(seller: i64) -> String {
    send_sql(SELLER_DELIVERY, OBLIGATION_2, "9002", 0)
        + &format!(
            "INSERT INTO public.consignment_dispatch_rounds
                (contract_id, obligation_id, round, round_id, delivery_id, rights_holder_identity,
                 bot_uid, world_id, subject, template, body_digest, round_ref, d0, credits, cod,
                 prior_round, authorized_by_intake, authorized_by_consumption)
             VALUES ('{CONTRACT}', '{OBLIGATION_2}', 1, '{SELLER_ROUND}', '{SELLER_DELIVERY}',
                     'FakeSellerOne', '12345', 'main', 'fake subject', 'consignment_cod',
                     decode(repeat('d1', 32), 'hex'), '{SELLER_ROUND}', '{PURCHASED_AT}', {seller},
                     0, NULL, NULL, NULL);
             INSERT INTO public.consignment_dispatch_attempts
                (delivery_id, fence, attempt_id, contract_id, obligation_id, round, leg,
                 started_at)
             VALUES ('{SELLER_DELIVERY}', 1, '{SELLER_ATTEMPT}', '{CONTRACT}', '{OBLIGATION_2}', 1,
                     'cash_payout', '{PURCHASED_AT}');"
        )
}

#[tokio::test]
async fn postgres_consignment_funding_refund_conversion_refuses_when_seller_payout_may_have_gone_out()
 {
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
    succeeds(pool, &seller_dispatch_sql(quote.seller_amount)).await;
    // 卖款已预留并发出：转换会让同一笔钱既付卖家又退买家，必须拒绝。
    for (byte, phase) in [(40, CashPhase::Reserve), (41, CashPhase::InFlight)] {
        store
            .test_settle_cash(SettleCash {
                mutation: mutation(byte),
                contract_id: id(CONTRACT),
                obligation_id: id(OBLIGATION_2),
                delivery_id: id(SELLER_DELIVERY),
                leg: CashLeg::Seller,
                phase,
                amount: quote.seller_amount,
            })
            .await
            .expect("卖款腿迁移");
    }
    let transfers = entries_of(pool, UID, "transfer").await;
    let refused = store
        .test_convert_refund(ConvertRefund {
            mutation: mutation(42),
            contract_id: id(CONTRACT),
            refund_obligation_id: id(OBLIGATION_3),
            price,
            quote,
        })
        .await;
    assert!(matches!(refused, Err(FundingError::Conflict)));
    assert_eq!(fee_fact_count(pool, "reversed").await, 0);
    assert_eq!(entries_of(pool, UID, "transfer").await, transfers, "拒绝时不转移任何金额");
    let refund_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_funding_allocations WHERE purpose = 'refund'",
    )
    .fetch_one(pool)
    .await
    .expect("读取退款分配");
    assert_eq!(refund_rows, 0, "没有建立退款分配");
}
