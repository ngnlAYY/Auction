//! 灭失释放占用：lost 只在同事务写入损失事实与损失证据时放行，挂单 loss_confirmed 关闭与之同提；
//! 无证据、证据对不上或回执缺事实一律被拒（原子性：命令、position、事实、案件、事件、占用、挂单全有全无）。
//!
//! 保护的不变量：2、3、6、8；损失事实不产生任何资金或处分代。

use super::super::contracts::fixture::seed_main_custody;
use super::super::deposit::support::{LISTING, POSITION, PROOF as MAIL_PROOF, fails, succeeds};
use super::super::ledger136::fixture::available_sql;
use super::command_sql::{
    claim_release, confirm_command, confirm_loss_sql, loss_facts, loss_listing, loss_proof_sql,
    lost_event, position_lost,
};
use super::{FACT, PROOF as LOSS_PROOF};
use crate::support::TestDatabase;

async fn claim_holder(pool: &sqlx::PgPool) -> Option<String> {
    sqlx::query_scalar(
        "SELECT position_id::text FROM public.consignment_equipment_claims
         WHERE world_id = 'main' AND canonical_eid = '42'",
    )
    .fetch_one(pool)
    .await
    .expect("读取占用")
}

#[tokio::test]
async fn postgres_consignment_recovery_lost_release_needs_same_transaction_fact_and_proof() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;
    // 证据由证据 owner 先行追加（独立命令）；确认事务只引用它。
    succeeds(pool, &loss_proof_sql()).await;

    // 反例：position 直接 lost，没有同事务的损失事实。
    fails(pool, &(confirm_command() + &position_lost()), "23514", Some("c7_position_loss_fact"))
        .await;
    // 反例：有 lost 事件与释放，但没有损失事实 / 证据（邮件证明冒充损失证据）：lost 事件的证明必须是同 position 的损失证据。
    fails(
        pool,
        &(confirm_command()
            + &loss_facts()
            + &position_lost()
            + &lost_event(MAIL_PROOF)
            + &claim_release(FACT)),
        "23514",
        Some("c7_custody_event_loss_proof"),
    )
    .await;
    // 反例：占用释放依据的不是损失事实（用 position 自身 ID 冒充）。
    fails(
        pool,
        &(confirm_command()
            + &loss_facts()
            + &position_lost()
            + &lost_event(LOSS_PROOF)
            + &claim_release(POSITION)),
        "23514",
        Some("c7_claim_loss_fact"),
    )
    .await;
    // 反例：回执在、事实与案件缺：不存在“回执在、事实缺”。
    fails(pool, &confirm_command(), "23514", Some("c7_command_loss_fact")).await;
    // 反例：损失事实在、释放与 lost 事件缺，挂单不能直接以 loss_confirmed 关闭（position 未 lost）。
    fails(
        pool,
        &(confirm_command() + &loss_facts() + &loss_listing()),
        "23514",
        Some("consignment_listing_transition"),
    )
    .await;
    assert_eq!(claim_holder(pool).await.as_deref(), Some(POSITION), "全部反例回滚，占用仍在");

    // 正例：命令、lost、事实与案件、lost 事件、释放、挂单关闭同一事务。
    succeeds(pool, &confirm_loss_sql()).await;
    assert_eq!(claim_holder(pool).await, None, "占用已释放");
    let (listing, reason, position, generation): (String, Option<String>, String, i64) = sqlx::query_as(
        "SELECT listing.state, listing.reason_code, position.state, position.current_generation
         FROM public.consignment_listings AS listing, public.consignment_custody_positions AS position
         WHERE listing.id = $1::uuid AND position.id = $2::uuid",
    )
    .bind(LISTING)
    .bind(POSITION)
    .fetch_one(pool)
    .await
    .expect("读取灭失结果");
    assert_eq!(
        (listing.as_str(), reason.as_deref(), position.as_str(), generation),
        ("closed", Some("loss_confirmed"), "lost", 1),
        "挂单关闭，position lost，处分代指针不变"
    );
    let generations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_disposition_generations")
            .fetch_one(pool)
            .await
            .expect("读取处分代数");
    assert_eq!(generations, 1, "灭失不新增处分代");
}

#[tokio::test]
async fn postgres_consignment_recovery_lost_release_rejects_a_loss_fact_from_an_earlier_transaction()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;
    succeeds(pool, &loss_proof_sql()).await;
    // 前一事务只提交命令、损失事实与案件，position 仍在库。
    succeeds(pool, &(confirm_command() + &loss_facts())).await;

    // 反例：本事务只推进 position / lost 事件 / 释放 / 挂单，引用前一事务的损失事实。
    fails(
        pool,
        &(position_lost() + &lost_event(LOSS_PROOF) + &claim_release(FACT) + &loss_listing()),
        "23514",
        Some("c7_position_loss_fact"),
    )
    .await;
    assert_eq!(claim_holder(pool).await.as_deref(), Some(POSITION), "反例回滚，占用仍在");
}
