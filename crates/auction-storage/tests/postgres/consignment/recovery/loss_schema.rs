//! 损失事实与案件：一个 position 至多一次损失终局；案件按 (position, 事实, 卖家) 唯一；人工结果有版本、
//! 每个结果至多一份卖家回执，且全部表没有任何金额、付款或发行列（不变量 6、8，决定 22）。

use super::super::deposit::support::{POSITION, USER_1, fails, succeeds};
use super::{
    CASE, FACT, PROOF, T0, case_sql, command_ids, fact_sql, loss_chain_sql, proof_sql, seed,
};
use crate::support::TestDatabase;

#[tokio::test]
async fn postgres_consignment_loss_allows_one_terminal_fact_per_position() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 5).await;

    // 正例：证据、事实、案件同事务落库。
    succeeds(pool, &loss_chain_sql(&commands)).await;
    let facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_loss_facts WHERE position_id = $1::uuid",
    )
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("读取损失事实");
    assert_eq!(facts, 1);

    // 反例：同一 position 再来一份独立证据与事实，被唯一约束拒绝（不是靠应用层先查后写）。
    let second_proof = "7c200000-0000-4000-8000-000000000002";
    let second_fact = "7c300000-0000-4000-8000-000000000002";
    fails(
        pool,
        &(proof_sql(second_proof, "fake-observation-2", &commands[3])
            + &fact_sql(second_fact, second_proof, &commands[4])),
        "23505",
        Some("c7_loss_fact_position_uq"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_loss_case_is_unique_per_fact_and_has_no_money_columns() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 4).await;
    succeeds(pool, &loss_chain_sql(&commands)).await;

    // 反例：同一 (position, 事实, 卖家) 不能开第二个案件。
    fails(
        pool,
        &case_sql("7c400000-0000-4000-8000-000000000002", FACT, &commands[3]),
        "23505",
        Some("c7_loss_case_key_uq"),
    )
    .await;
    // 反例：案件必须引用真实存在的损失事实。
    fails(
        pool,
        &case_sql(
            "7c400000-0000-4000-8000-000000000003",
            "7c300000-0000-4000-8000-0000000000ff",
            &commands[3],
        ),
        "23503",
        Some("c7_loss_case_fact_fk"),
    )
    .await;

    // 人工案件不授予自由现金写入权：五张损失表没有任何金额、付款、退款或发行列，缺列而非约定。
    let money_columns: Vec<String> = sqlx::query_scalar(
        "SELECT table_name || '.' || column_name FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name ~ '^consignment_loss_'
           AND column_name ~ '(amount|credit|price|fee|payment|refund|cash|issue|payout)'",
    )
    .fetch_all(pool)
    .await
    .expect("读取损失表列");
    assert!(money_columns.is_empty(), "损失表不得有资金列：{money_columns:?}");
}

#[tokio::test]
async fn postgres_consignment_loss_outcomes_are_versioned_and_receipted_once() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 5).await;
    succeeds(pool, &loss_chain_sql(&commands)).await;

    let opened = "7c700000-0000-4000-8000-000000000001";
    let reviewing = "7c700000-0000-4000-8000-000000000002";
    let outcome = |outcome_id: &str, version: i64, kind: &str, command: &str| {
        format!(
            "INSERT INTO public.consignment_loss_case_outcomes
                (outcome_id, case_id, case_version, outcome, command_id, occurred_at)
             VALUES ('{outcome_id}', '{CASE}', {version}, '{kind}', '{command}', '{T0}');"
        )
    };
    let receipt = |receipt_id: &str, outcome_id: &str| {
        format!(
            "INSERT INTO public.consignment_loss_receipts
                (receipt_id, case_id, outcome_id, seller_user_id, recorded_at)
             VALUES ('{receipt_id}', '{CASE}', '{outcome_id}',
                     '{USER_1}', '{T0}');"
        )
    };
    // 正例：版本 1 开案，版本 2 复核，各带一份回执。
    succeeds(
        pool,
        &(outcome(opened, 1, "opened", &commands[3])
            + &receipt("7c800000-0000-4000-8000-000000000001", opened)),
    )
    .await;
    succeeds(pool, &outcome(reviewing, 2, "reviewing", &commands[4])).await;
    // 反例：同一版本不能写两次；版本 1 之外不能再 opened。
    fails(
        pool,
        &outcome("7c700000-0000-4000-8000-000000000003", 2, "closed", &commands[2]),
        "23505",
        Some("c7_loss_outcome_version_uq"),
    )
    .await;
    fails(
        pool,
        &outcome("7c700000-0000-4000-8000-000000000004", 3, "opened", &commands[1]),
        "23514",
        Some("c7_loss_outcome_version_ck"),
    )
    .await;
    // 反例：同一结果不能有第二份卖家回执。
    fails(
        pool,
        &receipt("7c800000-0000-4000-8000-000000000002", opened),
        "23505",
        Some("c7_loss_receipt_outcome_uq"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_loss_proof_season_shape_follows_the_world() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let commands = command_ids(pool, 1).await;

    // 反例：main 世界没有季，带季的证据被命名 CHECK 拒绝。
    let with_season = proof_sql(PROOF, "fake-observation-1", &commands[0])
        .replace("'main', '42',", "'main', '42', 'fake-season',")
        .replace("canonical_eid, last_known_ref", "canonical_eid, season_id, last_known_ref");
    fails(pool, &with_season, "23514", Some("c7_loss_proof_season_ck")).await;
}
