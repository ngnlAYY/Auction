//! 周期：解禁原子关闭旧周期、开启新周期；迟到证据按发生时周期，旧周期只能得 cycle_closed。

use auction_domain::consignment::refusal::Qualification;
use uuid::Uuid;

use super::fixture::{
    buyer, command, count, intake, seed_intake_at, seed_pending_sanction, seller, submit,
};
use crate::support::TestDatabase;

async fn fact_cycle(owner: &sqlx::PgPool, intake_id: &str) -> Uuid {
    sqlx::query_scalar(
        "SELECT cycle_id FROM public.consignment_return_facts WHERE intake_id = $1::uuid",
    )
    .bind(intake_id)
    .fetch_one(owner)
    .await
    .expect("事实周期")
}

#[tokio::test]
async fn postgres_consignment_refusal_unblock_closes_cycle_and_late_evidence_is_cycle_closed() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let owner = owner.postgres_pool();
    let store = database.store().await.expect("以运行账号连接");
    let stock = seed_pending_sanction(owner, &store).await;
    // 解禁之后才发生的退回（未来时刻）与解禁之前发生但迟到的退回。
    seed_intakes_late(owner).await;

    // 解禁路径：旧周期关闭、新周期开启；pending 处罚转 cycle_closed，账号工作取消，新购阻断解除。
    let transition =
        store.test_refusal_close_cycle(command(100), buyer(), None).await.expect("关闭旧周期");
    assert_eq!(transition.closed_cycle_id, Some(stock.cycle_id));
    assert_eq!(transition.closed_sanction_id, Some(stock.sanction_id));
    assert_ne!(transition.opened_cycle_id, stock.cycle_id);
    let sanction_state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_sanctions WHERE id = $1")
            .bind(stock.sanction_id)
            .fetch_one(owner)
            .await
            .expect("处罚状态");
    assert_eq!(sanction_state, "cycle_closed");
    let work_state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_account_work WHERE id = $1")
            .bind(stock.account_work_id)
            .fetch_one(owner)
            .await
            .expect("工作状态");
    assert_eq!(work_state, "cancelled");
    let blocked =
        store.test_purchase_gate(command(101), buyer(), seller(), None).await.expect("阻断读");
    assert!(!blocked);

    // 迟到证据（解禁前发生）归旧周期：只留合格记录，不处罚、不阻断。
    let late = submit(&store, 4).await;
    assert_eq!(late.qualification, Qualification::Qualified);
    assert!(late.cycle_closed);
    assert!(late.sanction.is_none());
    assert_eq!(fact_cycle(owner, &intake(4).intake()).await, stock.cycle_id);
    assert_eq!(count(owner, "consignment_sanctions").await, 1);

    // 同请求重放返回原结果（不变量 1）：迟到证据仍是 cycle_closed，关闭前已记录的事实不是。
    let late_replay = submit(&store, 4).await;
    assert!(late_replay.replayed);
    assert_eq!(late_replay.qualification, Qualification::Qualified);
    assert!(late_replay.cycle_closed, "迟到证据重放必须保持 cycle_closed");
    assert!(late_replay.sanction.is_none());
    let early_replay = submit(&store, 1).await;
    assert!(early_replay.replayed);
    assert!(!early_replay.cycle_closed, "关闭前已记录的合格拒收重放不是 cycle_closed");
    assert_eq!(early_replay.sanction.map(|record| record.sanction_id), Some(stock.sanction_id));

    // 解禁后发生的退回归新周期，旧周期用过的拒收不再凑数。
    let fresh = submit(&store, 5).await;
    assert!(!fresh.cycle_closed);
    assert!(fresh.sanction.is_none());
    assert_eq!(fact_cycle(owner, &intake(5).intake()).await, transition.opened_cycle_id);
}

async fn seed_intakes_late(owner: &sqlx::PgPool) {
    // 第 4 个沿用购买时刻（早于解禁），第 5 个发生在遥远的未来（晚于解禁）。
    seed_intake_at(owner, 4, "2026-01-01T04:00:00Z").await;
    seed_intake_at(owner, 5, "2099-01-01T00:00:00Z").await;
}
