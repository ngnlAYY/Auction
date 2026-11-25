//! 解禁周期与处罚执行的衔接：周期已解禁的旧工作收口为 `cycle_closed`、不再禁人；解禁开启新周期，
//! 旧周期已落效果的处罚保持原状；解禁与执行器共用买家锁，竞争只有两种一致的结果。
//!
//! 保护的不变量：2（买家互斥）、6（不对解禁后的旧证再罚）、8（效果追加、历史不改写）。解禁入口对周期的接线
//! 归集成人；这里按 `reset_cycle` 系统命令的口径直接执行周期原语并读其事实。

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use auction_application::consignment::{
    SanctionWorkOutcome, SanctionWorkStore, reset_cycle_command,
};
use auction_domain::consignment::refusal::Qualification;

use super::super::refusal::fixture::{buyer, command, count, intake, seed_intake_at, submit};
use super::sanction_handoff::Env;

/// 解禁先于执行：旧工作收口为 cycle_closed，不禁人；重放不新增写入，新周期不阻断。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_cycle_closed_work_is_closed_without_blacklisting() {
    let env = Env::new().await;
    let transition = env
        .store
        .test_refusal_close_cycle(command(950), buyer(), None)
        .await
        .expect("解禁关闭旧周期");
    assert_eq!(transition.closed_sanction_id, Some(env.stock.sanction_id));
    // 解禁路径已把旧工作取消；执行器仍须把它收口成不可改写的 cycle_closed 事实。
    assert_eq!(env.store.open_sanction_work(10).await.expect("未收口工作"), vec![env.work()]);

    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("收口"),
        SanctionWorkOutcome::CycleClosed
    );
    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("重放"),
        SanctionWorkOutcome::Replayed
    );
    assert!(env.store.open_sanction_work(10).await.expect("未收口工作").is_empty());

    assert_eq!(
        env.scalar("SELECT state FROM public.consignment_sanction_effects").await,
        "cycle_closed"
    );
    assert_eq!(
        env.scalar(
            "SELECT (account_command_id IS NULL)::text FROM public.consignment_sanction_effects"
        )
        .await,
        "true",
        "收口没有账号命令"
    );
    assert_eq!(env.buyer_status().await, "active", "旧周期不再禁人");
    assert_eq!(env.blacklist_history().await, 0);
    assert!(!env.purchase_blocked(951).await, "新周期不阻断购买");
}

/// 解禁开启新周期：已落效果的旧处罚保持 applied，旧证不再凑数，新周期的退回从零计。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_unban_opens_new_cycle_and_keeps_applied_history() {
    let env = Env::new().await;
    // 解禁之后才发生的退回（遥远的未来时刻）归新周期。
    seed_intake_at(env.pool(), 4, "2099-01-01T00:00:00Z").await;
    let applied = env.store.execute_sanction_work(env.work()).await.expect("执行");
    assert!(matches!(applied, SanctionWorkOutcome::Applied { .. }));

    let transition = env
        .store
        .test_refusal_close_cycle(command(960), buyer(), None)
        .await
        .expect("解禁开启新周期");
    assert_eq!(transition.closed_cycle_id, Some(env.stock.cycle_id));
    assert_ne!(transition.opened_cycle_id, env.stock.cycle_id);
    assert_eq!(transition.closed_sanction_id, None, "已执行的处罚不再转移");
    assert_eq!(env.scalar("SELECT state FROM public.consignment_sanctions").await, "applied");
    assert_eq!(
        env.scalar("SELECT state FROM public.consignment_sanction_effects").await,
        "applied"
    );
    assert!(env.store.open_sanction_work(10).await.expect("未收口工作").is_empty());

    let fresh = submit(&env.store, 4).await;
    assert_eq!(fresh.qualification, Qualification::Qualified);
    assert!(!fresh.cycle_closed);
    assert!(fresh.sanction.is_none(), "旧周期用过的拒收不再凑数");
    assert_eq!(count(env.pool(), "consignment_sanctions").await, 1);
    let cycle: uuid::Uuid = sqlx::query_scalar(
        "SELECT cycle_id FROM public.consignment_return_facts WHERE intake_id = $1::uuid",
    )
    .bind(intake(4).intake())
    .fetch_one(env.pool())
    .await
    .expect("事实周期");
    assert_eq!(cycle, transition.opened_cycle_id);
}

/// 解禁与执行器共用买家锁：解禁持锁期间执行器必须等待（不能插在解禁与周期边界之间落账号效果），
/// 解禁提交后执行器只能把旧工作收口，账号不被禁。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_waits_for_unban_lock_then_closes_without_blacklisting()
 {
    let env = Env::new().await;
    let blocked = Arc::new(AtomicBool::new(false));
    let hook_store = Arc::clone(&env.store);
    let hook_blocked = Arc::clone(&blocked);
    let work = env.work();
    // 钩子在解禁已取得买家锁、尚未开快照时运行：此时执行器若不等锁就会在这里完成。
    let hook = move || -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let store = Arc::clone(&hook_store);
        let blocked = Arc::clone(&hook_blocked);
        Box::pin(async move {
            let attempt =
                tokio::time::timeout(Duration::from_millis(300), store.execute_sanction_work(work))
                    .await;
            blocked.store(attempt.is_err(), Ordering::SeqCst);
        })
    };
    env.store.test_refusal_close_cycle(command(970), buyer(), Some(&hook)).await.expect("解禁");
    assert!(blocked.load(Ordering::SeqCst), "解禁持锁期间执行器必须被买家锁挡住");
    assert_eq!(count(env.pool(), "consignment_sanction_effects").await, 0, "被挡住时没有任何效果");
    assert_eq!(env.blacklist_history().await, 0);

    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("解禁之后"),
        SanctionWorkOutcome::CycleClosed
    );
    assert_eq!(env.buyer_status().await, "active");
    assert_eq!(env.blacklist_history().await, 0);
    assert_eq!(
        env.scalar("SELECT state FROM public.consignment_sanction_effects").await,
        "cycle_closed"
    );
}

/// `reset_cycle` 系统命令：周期原语与账本回执同事务；同 ID 同请求回放原结果（不再开周期），
/// 同 ID 异请求冲突且不写第二行。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_reset_cycle_is_idempotent_and_conflicts_on_changed_request()
 {
    let env = Env::new().await;
    let unban_fact = uuid::Uuid::from_u128(0x7b30_0000_0000_4000_8000_0000_0000_0001);
    let (command_id, hash) = reset_cycle_command(buyer().as_uuid(), unban_fact);
    let first =
        env.store.test_reset_cycle(command_id, hash, buyer(), None).await.expect("解禁重置");
    assert_eq!(first.closed_cycle_id, Some(env.stock.cycle_id));
    assert_eq!(first.closed_sanction_id, Some(env.stock.sanction_id));
    let cycles_after = count(env.pool(), "consignment_refusal_cycles").await;

    // 同 ID 同请求：回放原结果，不再关闭或开启周期。
    let (_, same_hash) = reset_cycle_command(buyer().as_uuid(), unban_fact);
    let replay =
        env.store.test_reset_cycle(command_id, same_hash, buyer(), None).await.expect("回放");
    assert_eq!(replay, first);
    assert_eq!(count(env.pool(), "consignment_refusal_cycles").await, cycles_after);
    assert_eq!(
        env.scalar(
            "SELECT count(*)::text FROM public.consignment_commands WHERE operation = 'reset_cycle'"
        )
        .await,
        "1"
    );

    // 同 ID 异请求（摘要不同）：冲突，不写第二行、不动周期。
    let (_, other_hash) = reset_cycle_command(
        buyer().as_uuid(),
        uuid::Uuid::from_u128(0x7b30_0000_0000_4000_8000_0000_0000_0002),
    );
    assert!(env.store.test_reset_cycle(command_id, other_hash, buyer(), None).await.is_err());
    assert_eq!(count(env.pool(), "consignment_refusal_cycles").await, cycles_after);
    assert_eq!(
        env.scalar(
            "SELECT count(*)::text FROM public.consignment_commands WHERE operation = 'reset_cycle'"
        )
        .await,
        "1"
    );
    // 旧处罚随周期关闭转 cycle_closed，其工作由执行器收口并写账本回执。
    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("收口"),
        SanctionWorkOutcome::CycleClosed
    );
    assert_eq!(
        env.scalar("SELECT result_body->>'effect_state' FROM public.consignment_commands WHERE operation = 'apply_sanction'")
            .await,
        "cycle_closed"
    );
}
