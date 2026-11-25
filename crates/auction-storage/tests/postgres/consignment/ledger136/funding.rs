//! COD 实收分账：同一笔经济实收的 G = A + U 守恒、opening 份额不重叠且只消费一次、跨 source 守卫。

use sqlx::PgPool;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1};
use super::super::deposit::support::{check, fails, succeeds};
use super::fixture::{PAY_DELIVERY, PAY_MID, PURCHASED_AT, original_dispatch_sql, seed_purchased};
use crate::support::TestDatabase;

const UID: &str = "7001";
const RECEIPT_KEY: &str = "c6900000-0000-4000-8000-000000000001";
const OPENING_SOURCE: &str = "c6900000-0000-4000-8000-000000000002";
const PAYMENT_SOURCE: &str = "c6900000-0000-4000-8000-000000000003";
const PROPRIETARY: &str = "c6900000-0000-4000-8000-000000000004";
const TRANSIT: &str = "c6900000-0000-4000-8000-000000000005";
const SELLER: &str = "c6900000-0000-4000-8000-000000000006";
const FEE: &str = "c6900000-0000-4000-8000-000000000007";
const OPENING_ENTRY: &str = "c6900000-0000-4000-8000-000000000008";
const SLICE: &str = "c6900000-0000-4000-8000-000000000009";
const FEE_FACT: &str = "c6900000-0000-4000-8000-00000000000a";

/// 旧 opening：本 UID 已含 100，随后一个 opening 发行分录贷入自有用途。
fn opening_sql() -> String {
    format!(
        "INSERT INTO public.settlement_funding_sources (id, uid, kind, source_key, amount, proof_digest)
         VALUES ('{OPENING_SOURCE}', '{UID}', 'reconciliation', '{OPENING_SOURCE}', 100,
                 decode(repeat('01', 32), 'hex'));
         INSERT INTO public.settlement_funding_allocations (id, uid, purpose, amount)
         VALUES ('{PROPRIETARY}', '{UID}', 'proprietary', 100);
         INSERT INTO public.settlement_funding_entries
            (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
         VALUES ('{OPENING_ENTRY}', '{OPENING_SOURCE}', '{UID}', '{PROPRIETARY}',
                 '{OPENING_SOURCE}', 100, 'issue', decode(repeat('01', 32), 'hex'));"
    )
}

/// opening 份额：原 entry 区间 [from, to) 归属本实收业务键。
fn slice_sql(id: &str, from: i64, to: i64) -> String {
    format!(
        "INSERT INTO public.consignment_opening_slices
            (id, uid, receipt_key_id, boundary_at, opening_source_id, opening_issue_entry_id,
             original_allocation_id, amount_from, amount_to, basis)
         VALUES ('{id}', '{UID}', '{RECEIPT_KEY}', '{PURCHASED_AT}', '{OPENING_SOURCE}',
                 '{OPENING_ENTRY}', '{PROPRIETARY}', {from}, {to}, 'fixture');"
    )
}

fn allocations_sql() -> String {
    format!(
        "INSERT INTO public.settlement_funding_allocations
            (id, uid, purpose, consignment_contract_id, amount)
         VALUES ('{TRANSIT}', '{UID}', 'consignment_transit', '{CONTRACT}', 0),
                ('{SELLER}', '{UID}', 'seller', '{CONTRACT}', 0),
                ('{FEE}', '{UID}', 'fee', '{CONTRACT}', 0);"
    )
}

/// 付款 source（额度 U）、通用收据与寄售收据；G、A、U 由参数给出。
fn receipt_sql(gross: i64, included: i64, new_issue: i64) -> String {
    format!(
        "INSERT INTO public.settlement_funding_sources (id, uid, kind, source_key, amount, proof_digest)
         VALUES ('{PAYMENT_SOURCE}', '{UID}', 'collected_payment', '{PAYMENT_SOURCE}',
                 {new_issue}, decode(repeat('02', 32), 'hex'));
         INSERT INTO public.settlement_funding_receipts
            (source_id, uid, payment_obligation_id, delivery_id, external_mail_id, evidence)
         VALUES ('{PAYMENT_SOURCE}', '{UID}', '{OBLIGATION_1}', '{PAY_DELIVERY}', '{PAY_MID}',
                 '{{}}'::jsonb);
         INSERT INTO public.consignment_cod_receipts
            (source_id, uid, contract_id, payment_obligation_id, delivery_id, world_id, sent_mid,
             sent_fence, cod_leg, remote_payment_ref, receipt_key_id, gross, opening_included,
             new_issue)
         VALUES ('{PAYMENT_SOURCE}', '{UID}', '{CONTRACT}', '{OBLIGATION_1}', '{PAY_DELIVERY}',
                 'main', '{PAY_MID}', 1, 'original', NULL, '{RECEIPT_KEY}', {gross}, {included},
                 {new_issue});"
    )
}

fn use_sql() -> String {
    format!(
        "INSERT INTO public.consignment_opening_slice_uses (slice_id, receipt_source_id, receipt_key_id)
         VALUES ('{SLICE}', '{PAYMENT_SOURCE}', '{RECEIPT_KEY}');"
    )
}

/// 分录：新发行 U 贷入过渡用途、opening 借出 A，再由过渡用途转出 S、F。
fn entries_sql(issue: i64, borrow: i64, seller: i64, fee: i64) -> String {
    let entry = |kind: &str, debit: &str, credit: &str, source: &str, amount: i64, n: u32| {
        format!(
            "INSERT INTO public.settlement_funding_entries
                (id, command_id, uid, debit_id, credit_id, source_id, amount, kind, proof_digest)
             VALUES ('c6910000-0000-4000-8000-0000000000{n:02}', '{PAYMENT_SOURCE}', '{UID}',
                     {debit}, '{credit}', {source}, {amount}, '{kind}',
                     decode(repeat('03', 32), 'hex'));",
            debit = if debit.is_empty() { "NULL".to_owned() } else { format!("'{debit}'") },
            source = if source.is_empty() { "NULL".to_owned() } else { format!("'{source}'") },
        )
    };
    let mut sql = String::new();
    if issue > 0 {
        sql += &entry("issue", "", TRANSIT, PAYMENT_SOURCE, issue, 1);
    }
    if borrow > 0 {
        sql += &entry("transfer", PROPRIETARY, TRANSIT, "", borrow, 2);
    }
    sql += &entry("transfer", TRANSIT, SELLER, "", seller, 3);
    sql += &entry("transfer", TRANSIT, FEE, "", fee, 4);
    sql
}

fn funded_sql() -> String {
    format!(
        "INSERT INTO public.consignment_funded_sales
            (contract_id, uid, payment_obligation_id, receipt_source_id)
         VALUES ('{CONTRACT}', '{UID}', '{OBLIGATION_1}', '{PAYMENT_SOURCE}');
         INSERT INTO public.consignment_fee_facts
            (id, contract_id, kind, amount, receipt_source_id, fee_entry_id)
         VALUES ('{FEE_FACT}', '{CONTRACT}', 'earned', 5, '{PAYMENT_SOURCE}',
                 'c6910000-0000-4000-8000-000000000004');"
    )
}

/// 部分已含：P=101 的 main 付款，毛额 G=101，A=60 来自旧 opening，U=41 新发行；S=96、F=5。
/// 把 [`seed`] 的 Sale 推进到已付款：完整的 COD 实收分账（funded 投影、收据、分录与手续费事实同事务）。
pub(crate) async fn pay(pool: &PgPool) {
    succeeds(pool, &certify(101, 60, 41, 41, 60)).await;
}

fn certify(gross: i64, included: i64, new_issue: i64, issue: i64, borrow: i64) -> String {
    receipt_sql(gross, included, new_issue)
        + &allocations_sql()
        + &use_sql()
        + &entries_sql(issue, borrow, 96, 5)
        + &funded_sql()
}

/// 已有 opening 份额、尚未收款的 Sale（原件腿已首派）；其他测试需要已付款时再调 [`pay`]。
pub(crate) async fn seed(pool: &PgPool) {
    seed_purchased(pool).await;
    succeeds(pool, &original_dispatch_sql()).await;
    succeeds(pool, &(opening_sql() + &slice_sql(SLICE, 0, 60))).await;
}

async fn counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_cod_receipts),
                (SELECT count(*) FROM public.settlement_funding_entries WHERE id <> $1::uuid),
                (SELECT count(*) FROM public.consignment_funded_sales)",
    )
    .bind(OPENING_ENTRY)
    .fetch_one(pool)
    .await
    .expect("读取实收计数")
}

#[tokio::test]
async fn postgres_consignment_ledger136_cod_receipt_conserves_gross_opening_and_new_issue() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;

    // 反例：A > G、U != G - A 在行内被拒绝。
    check(pool, &receipt_sql(101, 102, 0), "c6_cod_receipt_split_ck").await;
    check(pool, &receipt_sql(101, 60, 40), "c6_cod_receipt_split_ck").await;
    // 反例：新发行少记一笔（U 未发行），提交时三等式失败，整笔回滚。
    fails(pool, &certify(101, 60, 41, 0, 60), "23514", Some("c6_cod_receipt_balance")).await;
    // 反例：份额合计 != A（份额只有 60，却声明 A=50），回滚。
    fails(pool, &certify(101, 50, 51, 51, 50), "23514", Some("c6_cod_receipt_balance")).await;
    // 反例：最终用途合计 != G（转出少于毛额）。
    fails(
        pool,
        &(receipt_sql(101, 60, 41)
            + &allocations_sql()
            + &use_sql()
            + &entries_sql(41, 60, 96, 4)
            + &funded_sql()),
        "23514",
        Some("c6_cod_receipt_balance"),
    )
    .await;
    assert_eq!(counts(pool).await.0, 0, "任一失败都不留收据");

    // 正例：G=101、A=60、U=41，S=96、F=5，总发行增量只有 41。
    succeeds(pool, &certify(101, 60, 41, 41, 60)).await;
    let (receipts, new_entries, funded) = counts(pool).await;
    assert_eq!((receipts, funded), (1, 1));
    assert_eq!(new_entries, 4, "一笔发行加三笔转移");
    let issued: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(amount), 0)::bigint FROM public.settlement_funding_entries
         WHERE kind = 'issue' AND source_id = $1::uuid",
    )
    .bind(PAYMENT_SOURCE)
    .fetch_one(pool)
    .await
    .expect("读取付款 source 发行额");
    assert_eq!(issued, 41);

    // 同一经济实收（原 UID、世界、原付款 Sent MID、COD 腿）只有一份结果。
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_cod_receipts
                (source_id, uid, contract_id, payment_obligation_id, delivery_id, world_id,
                 sent_mid, sent_fence, cod_leg, remote_payment_ref, receipt_key_id, gross,
                 opening_included, new_issue)
             VALUES ('c6900000-0000-4000-8000-0000000000f1', '{UID}', '{CONTRACT}',
                     'c6900000-0000-4000-8000-0000000000f2',
                     'c6900000-0000-4000-8000-0000000000f3', 'main', '{PAY_MID}', 1, 'original',
                     NULL, 'c6900000-0000-4000-8000-0000000000f4', 101, 0, 101);"
        ),
        "23505",
        Some("c6_cod_receipt_key_uq"),
    )
    .await;
    // 每段 opening 份额只消费一次。
    fails(pool, &use_sql(), "23505", Some("c6_slice_use_pk")).await;
}

#[tokio::test]
async fn postgres_consignment_ledger136_opening_slices_never_overlap_and_opening_cannot_issue_directly()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;

    // 反例：同一 opening 发行分录的区间不得相交（[50,80) 与已有 [0,60)）。
    fails(
        pool,
        &slice_sql("c6900000-0000-4000-8000-0000000000b1", 50, 80),
        "23514",
        Some("c6_slice_overlap"),
    )
    .await;
    // 反例：区间越过原正额（entry 只有 100）。
    fails(
        pool,
        &slice_sql("c6900000-0000-4000-8000-0000000000b2", 90, 120),
        "23514",
        Some("c6_slice_entry_range"),
    )
    .await;
    // 正例：紧邻的 [60,100) 可以登记。
    succeeds(pool, &slice_sql("c6900000-0000-4000-8000-0000000000b3", 60, 100)).await;

    // 反例：opening（reconciliation）来源不得直接向寄售合同用途发行，只能经 transfer 借出；
    // 跨 source 守卫先于额度检查，拒绝原因不是额度。
    fails(
        pool,
        &(allocations_sql()
            + &format!(
                "INSERT INTO public.settlement_funding_entries
                    (id, command_id, uid, credit_id, source_id, amount, kind, proof_digest)
                 VALUES ('c6910000-0000-4000-8000-0000000000e1', '{OPENING_SOURCE}', '{UID}',
                         '{TRANSIT}', '{OPENING_SOURCE}', 10, 'issue',
                         decode(repeat('04', 32), 'hex'));"
            )),
        "23514",
        Some("c6_issue_cross_source"),
    )
    .await;
}
