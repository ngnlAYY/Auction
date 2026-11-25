//! 已收款成交的共享夹具（原“退款调用”入口已删除：退款转换只经 Admin 退款类处置与退回处置的
//! `refund_p` 效果，同事务调用 6B 单写者 `convert_refund_in_tx`，两者的用例在各自目录）。

use auction_domain::consignment::{FeeBps, ListingPrice, ListingQuote, quote_v1};
use sqlx::PgPool;

use super::super::super::contracts::fixture::{
    CONTRACT, OBLIGATION_1, OBLIGATION_2, seed_main_custody,
};
use super::super::super::deposit::support::succeeds;
use super::super::super::funding::fixture::{UID, id};
use super::super::super::ledger136::fixture::{
    PAY_DELIVERY, PAY_MID, PRICE, PURCHASE_COMMAND, available_sql, original_dispatch_sql,
    purchase_command, purchase_listing, purchase_position, sale_graph,
};
use crate::support::TestDatabase;

const RECEIPT_KEY: &str = "c7f30000-0000-4000-8000-000000000001";
const PAYMENT_SOURCE: &str = "c7f30000-0000-4000-8000-000000000002";
const TRANSIT: &str = "c7f30000-0000-4000-8000-000000000003";
const SELLER: &str = "c7f30000-0000-4000-8000-000000000004";
const FEE: &str = "c7f30000-0000-4000-8000-000000000005";
const FEE_ENTRY: &str = "c7f40000-0000-4000-8000-000000000003";
pub(in crate::consignment) const PAYOUT_DELIVERY: &str = "c6520000-0000-4000-8000-000000000001";
pub(in crate::consignment) const PAYOUT_ROUND: &str = "c6620000-0000-4000-8000-000000000001";
pub(in crate::consignment) const PAYOUT_ATTEMPT: &str = "c6720000-0000-4000-8000-000000000001";
pub(in crate::consignment) const PAYOUT_MID: &str = "9002";
const FEE_AMOUNT: i64 = 5;

/// 一份 funded 的 main 成交：P=101、F=5、S=96，全部为新发行；含 funded 投影（F>0 需随同 earned 写入）。
fn funded_sql() -> String {
    let seller = PRICE - FEE_AMOUNT;
    format!(
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
                ('{SELLER}', '{UID}', 'seller', '{OBLIGATION_2}', '{CONTRACT}', {seller}),
                ('{FEE}', '{UID}', 'fee', NULL, '{CONTRACT}', {FEE_AMOUNT});
         INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, debit_id, credit_id, source_id, amount, kind, proof_digest)
         VALUES ('c7f40000-0000-4000-8000-000000000001', '{PAYMENT_SOURCE}', '{UID}', NULL,
                 '{TRANSIT}', '{PAYMENT_SOURCE}', {PRICE}, 'issue', decode(repeat('03', 32), 'hex')),
                ('c7f40000-0000-4000-8000-000000000002', '{PAYMENT_SOURCE}', '{UID}', '{TRANSIT}',
                 '{SELLER}', NULL, {seller}, 'transfer', decode(repeat('03', 32), 'hex')),
                ('{FEE_ENTRY}', '{PAYMENT_SOURCE}', '{UID}', '{TRANSIT}', '{FEE}', NULL,
                 {FEE_AMOUNT}, 'transfer', decode(repeat('03', 32), 'hex'));
         INSERT INTO public.consignment_funded_sales
            (contract_id, uid, payment_obligation_id, receipt_source_id)
         VALUES ('{CONTRACT}', '{UID}', '{OBLIGATION_1}', '{PAYMENT_SOURCE}');"
    )
}

/// 在已购的存量上补写“原件腿已首派且已收款（funded）”的事实：Sale 仍开放，买家已付款。
/// 供灭失确认的“已付款买家责任”反例使用；`owner` 是维护账号连接，`store` 是运行账号门面。
pub(in crate::consignment::recovery) async fn fund_open_sale(
    owner: &PgPool,
    store: &auction_storage::DatabaseStore,
) {
    succeeds(owner, &original_dispatch_sql()).await;
    let (_, quote) = quote();
    store
        .test_write_fee_earned(
            &funded_sql(),
            id(CONTRACT),
            id(PAYMENT_SOURCE),
            id(FEE_ENTRY),
            quote,
        )
        .await
        .expect("记录 earned");
}

fn quote() -> (ListingPrice, ListingQuote) {
    let price = ListingPrice::new(PRICE).expect("价格");
    (price, quote_v1(price, FeeBps::new(500).expect("费率")))
}

/// 已购、原件腿已首派、且已收款（funded）的成交；返回库与门面。
pub(in crate::consignment) async fn funded_sale()
-> (TestDatabase, PgPool, auction_storage::DatabaseStore) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool().clone();
    let store = database.store().await.expect("以运行账号连接");
    seed_main_custody(&pool).await;
    succeeds(&pool, &available_sql()).await;
    let mut graph = sale_graph();
    graph.fee = FEE_AMOUNT;
    graph.seller_amount = PRICE - FEE_AMOUNT;
    graph.payout_amount = PRICE - FEE_AMOUNT;
    succeeds(
        &pool,
        &(purchase_command()
            + &graph.sql(PURCHASE_COMMAND)
            + &purchase_position()
            + &purchase_listing()),
    )
    .await;
    succeeds(&pool, &original_dispatch_sql()).await;
    fund_the_sale(&store).await;
    (database, pool, store)
}

/// 给已购且原件腿已首派的成交写入收款事实（funded 投影与手续费 earned）。
pub(in crate::consignment) async fn fund_the_sale(store: &auction_storage::DatabaseStore) {
    let (_, quote) = quote();
    store
        .test_write_fee_earned(
            &funded_sql(),
            id(CONTRACT),
            id(PAYMENT_SOURCE),
            id(FEE_ENTRY),
            quote,
        )
        .await
        .expect("记录 earned");
}

/// 分配金额；该用途的分配行尚不存在时按 0 计。
pub(in crate::consignment) async fn allocation(pool: &PgPool, purpose: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT amount FROM public.settlement_funding_allocations
         WHERE uid = $1 AND purpose = $2 AND consignment_contract_id = $3",
    )
    .bind(UID)
    .bind(purpose)
    .bind(id(CONTRACT))
    .fetch_optional(pool)
    .await
    .expect("读取分配")
    .unwrap_or(0)
}

pub(in crate::consignment) async fn closures(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_sale_closures")
        .fetch_one(pool)
        .await
        .expect("封闭事实")
}
