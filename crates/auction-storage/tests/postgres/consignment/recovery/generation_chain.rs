//! 处分代 g+1：撤回代与重列代在同事务命令（与合同）下写入，跳代、重复后继、缺命令或缺合同都被拒。
//!
//! 保护的不变量：2（唯一后继、同事务锁内写入）、3（命令、代、合同、事件、版本同提）、6（失败关闭）。

use super::super::contracts::fixture::{
    CONTRACT, Graph, OBLIGATION_1, OBLIGATION_2, OBLIGATION_3, seed_main_custody,
};
use super::super::deposit::support::{DECIDE, POSITION, fails, succeeds};
use super::super::ledger136::fixture::available_sql;
use super::command_sql::{
    HOLD_COMMAND, RELIST_COMMAND, WITHDRAW_COMMAND, generation, hold_sql, pointer, relist_command,
    relist_listing, relist_sql, withdraw_command, withdraw_listing, withdraw_sql,
};
use crate::support::TestDatabase;

const OTHER_CONTRACT: &str = "c5000000-0000-4000-8000-0000000000b1";
const OTHER_COD: &str = "c5100000-0000-4000-8000-0000000000b1";
const OTHER_PAYOUT: &str = "c5100000-0000-4000-8000-0000000000b2";
const OTHER_REFUND: &str = "c5100000-0000-4000-8000-0000000000b3";

#[tokio::test]
async fn postgres_consignment_recovery_withdrawal_generation_needs_contract_and_chain() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &available_sql()).await;

    // 反例：撤回代没有同事务的 Withdrawal 合同。
    fails(
        pool,
        &(withdraw_command()
            + &generation(2, "withdrawal_only", 1, WITHDRAW_COMMAND)
            + &withdraw_listing()),
        "23514",
        Some("c7_generation_successor_contract"),
    )
    .await;
    // 反例：跳代（g+2 直接接 g）与首代目的混用。
    fails(
        pool,
        &(withdraw_command() + &generation(3, "withdrawal_only", 1, WITHDRAW_COMMAND)),
        "23514",
        Some("c4_generation_number_ck"),
    )
    .await;
    fails(
        pool,
        &(withdraw_command() + &generation(2, "first_sale", 1, WITHDRAW_COMMAND)),
        "23514",
        Some("c7_generation_purpose_ck"),
    )
    .await;

    // 正例：命令、g+1 撤回代、带前驱代引用的 Withdrawal 合同、事件与挂单版本同事务。
    succeeds(pool, &withdraw_sql()).await;
    let (generations, purpose, state): (i64, String, String) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_disposition_generations),
                (SELECT purpose FROM public.consignment_disposition_generations WHERE generation = 2),
                (SELECT state FROM public.consignment_listings)",
    )
    .fetch_one(pool)
    .await
    .expect("读取撤回结果");
    assert_eq!(
        (generations, purpose.as_str(), state.as_str()),
        (2, "withdrawal_only", "withdrawal_pending")
    );

    // 反例：重复后继（同一前驱再接一个第 2 代）被主键 / 唯一后继拒绝。
    fails(
        pool,
        &(relist_command() + &generation(2, "relist", 1, RELIST_COMMAND)),
        "23505",
        Some("c4_generation_pk"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_recovery_relist_generation_needs_the_relist_command() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &hold_sql()).await;

    // 反例：重列代引用的命令不是本事务写入的成功重列命令（这里借用已提交的止售命令）。
    fails(
        pool,
        &generation(2, "relist", 1, HOLD_COMMAND),
        "23514",
        Some("c7_generation_successor_command"),
    )
    .await;
    // 反例：命令与重列版本都在，但没有 g+1 重列代：挂单版本缺同事务的重列代。
    fails(
        pool,
        &(relist_command() + &relist_listing()),
        "23514",
        Some("consignment_listing_version_event"),
    )
    .await;
    // 正例：命令、g+1 重列代、relisted 事件与挂单版本同事务。
    succeeds(pool, &relist_sql()).await;
    let (generation_count, state): (i64, String) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.consignment_disposition_generations),
                (SELECT state FROM public.consignment_listings)",
    )
    .fetch_one(pool)
    .await
    .expect("读取重列结果");
    assert_eq!((generation_count, state.as_str()), (2, "available"));
}

async fn pointer_state(pool: &sqlx::PgPool) -> (i64, i64) {
    sqlx::query_as("SELECT current_generation, version FROM public.consignment_custody_positions")
        .fetch_one(pool)
        .await
        .expect("读取 position 指针")
}

#[tokio::test]
async fn postgres_consignment_recovery_position_pointer_only_advances_to_the_same_transaction_successor()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    succeeds(pool, &hold_sql()).await;

    // 反例：没有后继行，指针直接前进到 2。
    fails(pool, &pointer(2, 3), "23514", Some("c7_position_generation_pointer")).await;
    // 反例：后继行在，但指针跳代（1 -> 3）。
    fails(
        pool,
        &(relist_command()
            + &generation(2, "relist", 1, RELIST_COMMAND)
            + &generation(3, "relist", 2, RELIST_COMMAND)
            + &pointer(3, 3)),
        "23514",
        Some("c7_position_generation_pointer"),
    )
    .await;
    // 反例：后继行在，但指针没有同事务推进（后继行成为孤儿）。
    fails(
        pool,
        &(relist_command() + &generation(2, "relist", 1, RELIST_COMMAND) + &relist_listing()),
        "23514",
        Some("c7_generation_successor_pointer"),
    )
    .await;
    // 正例：命令、g+1 重列代、指针推进、事件与挂单版本同事务，指针前进到 g+1。
    succeeds(pool, &relist_sql()).await;
    assert_eq!(pointer_state(pool).await, (2, 3), "重列后指针在 g+1");
    // 反例：回退指针（2 -> 1）。
    fails(pool, &pointer(1, 4), "23514", Some("c7_position_generation_pointer")).await;
    // 反例：再次前进到 3，但 3 号后继行不在本事务（只有已提交的 2 号行）。
    fails(pool, &pointer(3, 4), "23514", Some("c7_position_generation_pointer")).await;
    assert_eq!(pointer_state(pool).await, (2, 3), "全部反例回滚，指针仍在 g+1");
}

/// 来源兼容（D15）：义务的前置必须在同一合同内。另一合同已提交的同类义务不能充当本合同的前置，
/// 提交时被合同图守卫整体拒绝；同一写序改用本合同的前置即可提交（不变量 6）。
#[tokio::test]
async fn postgres_consignment_obligation_prerequisite_cannot_cross_contracts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_main_custody(pool).await;
    // 合同 A：第 1 代的 Sale 图，OBLIGATION_1 是它的付款义务。
    succeeds(pool, &Graph::sale_main().sql(DECIDE)).await;
    succeeds(pool, &hold_sql()).await;
    succeeds(pool, &relist_sql()).await;

    // 合同 B：第 2 代（重列代）上的 Sale 图，换用独立的合同与义务 ID。
    let head = format!("'{POSITION}', 1,");
    let own = Graph::sale_main().sql(RELIST_COMMAND);
    assert_eq!(own.matches(&head).count(), 1, "夹具形状变了");
    let own = own
        .replace(&head, &format!("'{POSITION}', 2,"))
        .replace(CONTRACT, OTHER_CONTRACT)
        .replace(OBLIGATION_1, OTHER_COD)
        .replace(OBLIGATION_2, OTHER_PAYOUT)
        .replace(OBLIGATION_3, OTHER_REFUND);
    let own_prerequisite = format!("'{OTHER_COD}', '{OTHER_CONTRACT}', 'sale'");
    assert_eq!(own.matches(&own_prerequisite).count(), 2, "夹具形状变了");
    let crossed =
        own.replace(&own_prerequisite, &format!("'{OBLIGATION_1}', '{OTHER_CONTRACT}', 'sale'"));
    // 反例：B 的卖款与退款以 A 的付款义务（同为 winner_equipment_cod）为前置。
    fails(pool, &crossed, "23514", Some("c5_contract_graph")).await;
    let contracts: i64 = sqlx::query_scalar("SELECT count(*) FROM public.consignment_contracts")
        .fetch_one(pool)
        .await
        .expect("读取合同数");
    assert_eq!(contracts, 1, "被拒的写序不得留下合同 B");
    // 正例：同一写序以本合同的付款义务为前置即可提交。
    succeeds(pool, &own).await;
}
