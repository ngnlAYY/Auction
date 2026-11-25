//! Admin 解禁接上寄售周期边界（S2-1）：解禁与周期重置同事务、与处罚执行器共用买家锁、重放不再重置；
//! 以及寄售存量探针把待执行处罚的账号工作算作存量。
//!
//! 保护的不变量：1（重放回原结果）、2（买家互斥）、3（解禁与周期边界同提交）、5（处罚工作是 Worker 存量）。

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::admin::{
    AdminUserMutationAction, AdminUserMutationWrite, AdminUserStatus, AdminUserStore,
};
use auction_application::consignment::{
    PaymentReminderService, PaymentReminderStore, SanctionWorkOutcome, SanctionWorkStore,
};
use auction_application::core::RequestHash;
use auction_time::Timestamp;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::original_dispatch_sql;
use super::super::ledger136::purchase::buyer_claimed_original_sql;
use super::super::refusal::fixture::{buyer, command};
use super::sanction_handoff::Env;
use crate::support::TestDatabase;

fn admin_command(seed: u128) -> Uuid {
    Uuid::from_u128(0x5210_0000_0000_4000_8000_0000_0000_0000 + seed)
}

fn mutation(action: AdminUserMutationAction, seed: u128, version: i64) -> AdminUserMutationWrite {
    AdminUserMutationWrite {
        command_id: admin_command(seed),
        request_hash: RequestHash::new([u8::try_from(seed).expect("种子"); 32]),
        user_id: buyer().as_uuid(),
        expected_state_version: version,
        action,
        username: None,
        forum_account_id: None,
        public_reason: (action == AdminUserMutationAction::Blacklist)
            .then(|| "fake admin reason".to_owned()),
        key: None,
        actor_id: "fake-admin".to_owned(),
        request_id: None,
        now: Timestamp::now(),
    }
}

async fn user_version(env: &Env) -> i64 {
    sqlx::query_scalar("SELECT state_version FROM public.users WHERE id = $1")
        .bind(buyer().as_uuid())
        .fetch_one(env.pool())
        .await
        .expect("用户版本")
}

async fn count(env: &Env, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(env.pool()).await.expect("计数")
}

/// 先由 Admin 以别的理由拉黑买家（存量里的处罚仍待执行），返回拉黑后的用户版本。
async fn blacklist_first(env: &Env) -> i64 {
    let version = user_version(env).await;
    env.store
        .mutate(mutation(AdminUserMutationAction::Blacklist, 1, version))
        .await
        .expect("Admin 拉黑");
    user_version(env).await
}

/// 正例：解禁同事务关闭旧周期（待执行处罚转 cycle_closed）、开启新周期并写 reset_cycle 回执；
/// 之后执行器只能把旧工作收口、不再禁人；同命令重放返回原结果，不再重置。
#[tokio::test]
async fn postgres_consignment_recovery_admin_unban_resets_the_cycle_in_the_same_transaction() {
    let env = Env::new().await;
    let version = blacklist_first(&env).await;
    let reset_rows =
        "SELECT count(*) FROM public.consignment_commands WHERE operation = 'reset_cycle'";
    assert_eq!(count(&env, reset_rows).await, 0);

    env.store
        .mutate(mutation(AdminUserMutationAction::Unblacklist, 2, version))
        .await
        .expect("解禁");

    assert_eq!(env.buyer_status().await, "active");
    assert_eq!(count(&env, reset_rows).await, 1, "周期重置回执与解禁同提交");
    assert_eq!(env.scalar("SELECT state FROM public.consignment_sanctions").await, "cycle_closed");
    let closed: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_refusal_cycles WHERE id = $1")
            .bind(env.stock.cycle_id)
            .fetch_one(env.pool())
            .await
            .expect("旧周期");
    assert_eq!(closed, "closed");
    let open_cycles = "SELECT count(*) FROM public.consignment_refusal_cycles WHERE state = 'open'";
    assert_eq!(count(&env, open_cycles).await, 1, "新周期已开启");
    let cycles = "SELECT count(*) FROM public.consignment_refusal_cycles";
    let cycles_after = count(&env, cycles).await;

    // 解禁之后执行器只能收口旧工作：账号保持 active，黑名单历史只有 Admin 自己那一条。
    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("执行"),
        SanctionWorkOutcome::CycleClosed
    );
    assert_eq!(env.buyer_status().await, "active");
    assert_eq!(env.blacklist_history().await, 1);

    // 重放：同命令同请求返回原结果，不再关闭或开启周期，不写第二条回执。
    env.store
        .mutate(mutation(AdminUserMutationAction::Unblacklist, 2, version))
        .await
        .expect("重放");
    assert_eq!(count(&env, reset_rows).await, 1);
    assert_eq!(count(&env, cycles).await, cycles_after);
}

/// 解禁与处罚执行器、拒收共用同一把买家锁：另一事务持锁期间解禁停在买家锁上等待（以 `pg_stat_activity`
/// 观测到 advisory 锁等待为准，不按时长判断），锁释放后解禁完成。
#[tokio::test]
async fn postgres_consignment_recovery_admin_unban_waits_for_the_buyer_lock() {
    let env = Env::new().await;
    let version = blacklist_first(&env).await;
    let pending: Arc<Mutex<Option<JoinHandle<_>>>> = Arc::new(Mutex::new(None));
    let (hook_store, hook_pool, hook_pending) =
        (Arc::clone(&env.store), env.pool().clone(), Arc::clone(&pending));
    // 钩子在拒收原语已取得买家锁、尚未开快照时运行：解禁在另一任务里提交，确认它停在 advisory 锁上再放行。
    let hook = move || -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let (store, pool, pending) =
            (Arc::clone(&hook_store), hook_pool.clone(), Arc::clone(&hook_pending));
        Box::pin(async move {
            let unban = tokio::spawn(async move {
                store.mutate(mutation(AdminUserMutationAction::Unblacklist, 3, version)).await
            });
            wait_for_an_advisory_lock_wait(&pool).await;
            assert!(!unban.is_finished(), "持买家锁期间解禁不得完成");
            *pending.lock().expect("任务槽") = Some(unban);
        })
    };
    env.store.test_refusal_close_cycle(command(980), buyer(), Some(&hook)).await.expect("持锁");
    let unban = pending.lock().expect("任务槽").take().expect("解禁任务");
    unban.await.expect("解禁任务结束").expect("锁释放后解禁完成");
    assert_eq!(env.buyer_status().await, "active");
}

/// 轮询到有后端在等 advisory 锁，带上限；取代按时长判断“已阻塞”。
async fn wait_for_an_advisory_lock_wait(pool: &sqlx::PgPool) {
    for _ in 0..500 {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                            WHERE datname = current_database() AND wait_event_type = 'Lock'
                              AND wait_event = 'advisory' AND pid <> pg_backend_pid())",
        )
        .fetch_one(pool)
        .await
        .expect("读取等待状态");
        if waiting {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等待上限内没有后端进入 advisory 锁等待");
}

/// 其余存量都已收口时，待执行处罚的账号工作仍让探针为真；执行器收口后探针为假。
///
/// 挂单与 position 经真实写序终结：原件腿首派后买家确领原件（确领证明、position handed_over、占用释放
/// 与挂单 closed/sold 同事务，全部经守卫）；接管行的 Take 投递以失败收口。
#[tokio::test]
async fn postgres_consignment_lookup_probe_counts_pending_sanction_work() {
    let env = Env::new().await;
    succeeds(env.pool(), &(original_dispatch_sql() + &buyer_claimed_original_sql())).await;
    succeeds(
        env.pool(),
        "UPDATE public.deliveries
         SET status = 'failed', completed_at = clock_timestamp(), failure_code = 'fake_closed_work'
         WHERE id IN (SELECT delivery_id FROM public.mail_disposition_work);",
    )
    .await;
    // 付款期限已过、首派已证寄出的合同也是恢复存量（探针与 `due_reminders` 同判据）：先由提醒写端作出
    // 每合同唯一的决定，余下才只剩待执行处罚。
    let reminders: Arc<dyn PaymentReminderStore> = env.store.clone();
    PaymentReminderService::new(reminders)
        .remind(CONTRACT.parse().expect("合同 ID"))
        .await
        .expect("提醒决定");
    assert!(env.store.has_consignment_work().await.expect("探针"), "只剩待执行处罚仍是存量");

    let outcome = env.store.execute_sanction_work(env.work()).await.expect("执行");
    assert!(matches!(outcome, SanctionWorkOutcome::Applied { .. }), "{outcome:?}");
    assert!(!env.store.has_consignment_work().await.expect("探针"), "处罚收口后无存量");
}

/// 反例：没有任何寄售记录的用户解禁走原路径：不开周期、不写寄售账本，拍卖侧的解禁结果不变。
#[tokio::test]
async fn postgres_consignment_recovery_unban_without_consignment_history_touches_no_consignment_rows()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    let user = Uuid::from_u128(0x5211_0000_0000_4000_8000_0000_0000_0001);
    succeeds(
        pool,
        &format!(
            "INSERT INTO public.users (id, username, public_key_hash)
             VALUES ('{user}', 'FakeAuctionOnly', decode(repeat('16', 32), 'hex'));"
        ),
    )
    .await;
    let version = || async {
        sqlx::query_scalar::<_, i64>("SELECT state_version FROM public.users WHERE id = $1")
            .bind(user)
            .fetch_one(pool)
            .await
            .expect("用户版本")
    };
    let mut blacklist = mutation(AdminUserMutationAction::Blacklist, 4, version().await);
    blacklist.user_id = user;
    store.mutate(blacklist).await.expect("拉黑");
    let consignment_rows = "SELECT (SELECT count(*) FROM public.consignment_refusal_cycles)
                                 + (SELECT count(*) FROM public.consignment_commands)";
    let before: i64 = sqlx::query_scalar(consignment_rows).fetch_one(pool).await.expect("寄售行");

    let mut unban = mutation(AdminUserMutationAction::Unblacklist, 5, version().await);
    unban.user_id = user;
    let result = store.mutate(unban).await.expect("解禁");

    assert_eq!(result.user.status, AdminUserStatus::Active);
    let after: i64 = sqlx::query_scalar(consignment_rows).fetch_one(pool).await.expect("寄售行");
    assert_eq!((before, after), (0, 0), "寄售表零写入");
    let revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM public.user_blacklist WHERE user_id = $1",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .expect("黑名单");
    assert!(revoked, "拍卖侧的解禁照常撤销黑名单");
}
