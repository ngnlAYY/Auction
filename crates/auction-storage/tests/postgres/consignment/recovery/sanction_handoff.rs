//! 第 6 到 7 期的处罚交接（本期头号验收）：第 6 期写端造的三份合格退回、待执行处罚与账号工作，
//! 在窗口早已滑过之后由第 7 期执行器接管。
//!
//! 存量按 D9 §11.1 字面：同买家三张不同 isekai 合同的合格未付载体退回，最早一份在 t0 时为 89 天，第三份在 t0
//! 首次获证合格、Take 待核（`refusal::stock`）；t0+2 天接管时按当天窗口只剩两份。
//!
//! 保护的不变量：1（账号效果恰一次，重放 / 重启 / 并发领取不变）、2（执行与解禁共用买家锁）、
//! 6（不重数窗口、不补写矛盾事实）、8（效果追加）。夹具只用 6D3 的存量构造器与真实写入口，不卸触发器、不删约束。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::admin::RecoveryBuyerBlacklister;
use auction_application::consignment::{
    ConsignmentSanctionAccountWork, SanctionWorkOutcome, SanctionWorkRef, SanctionWorkStore,
    consignment_sanction_blacklist_command,
};
use auction_domain::consignment::refusal::SanctionRule;
use auction_jobs::consignment::{SanctionExecutionRuntime, run_sanction_execution};
use auction_observability::WorkerResult;
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::PgPool;
use tokio::sync::{Notify, watch};
use uuid::Uuid;

use super::super::refusal::fixture::{
    PendingSanctionStock, buyer, command, count, seed_pending_sanction, seller,
};
use super::super::refusal::stock::{
    IsekaiStock, OWN, ThirdReturn, own, principal_effects, seed_isekai_sanction,
};
use crate::support::TestDatabase;

/// 一个克隆库、维护账号、运行账号与第 6 期造好的存量。
pub(super) struct Env {
    pub(super) database: TestDatabase,
    pub(super) owner: DatabaseStore,
    pub(super) store: Arc<DatabaseStore>,
    pub(super) stock: PendingSanctionStock,
    /// D9 字面的 isekai 跨界存量（只有 [`Env::isekai`] 构造）。
    pub(super) isekai: Option<IsekaiStock>,
}

impl Env {
    /// main 的“达阈值待执行处罚”存量（6D3）；周期与解禁例在它上面继续追加 main 退件。
    pub(super) async fn new() -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let store = Arc::new(database.store().await.expect("以运行账号连接"));
        let stock = seed_pending_sanction(owner.postgres_pool(), &store).await;
        Self { database, owner, store, stock, isekai: None }
    }

    /// 第 6→7 期跨界头号验收例的存量（D9 §11.1）：三张不同 isekai 合同的合格未付载体退回，最早一份在 t0 时为
    /// 89 天，第三份季后补证首次合格、Take 待核。
    pub(super) async fn isekai() -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let owner = database.maintenance_store().await.expect("以维护账号连接");
        let store = Arc::new(database.store().await.expect("以运行账号连接"));
        let mut isekai =
            seed_isekai_sanction(owner.postgres_pool(), &store, ThirdReturn::BeforeClose).await;
        let stock = isekai.sanction.take().expect("第三份补证应触发处罚");
        Self { database, owner, store, stock, isekai: Some(isekai) }
    }

    pub(super) fn pool(&self) -> &PgPool {
        self.owner.postgres_pool()
    }

    pub(super) fn work(&self) -> SanctionWorkRef {
        SanctionWorkRef {
            work_id: self.stock.account_work_id,
            sanction_id: self.stock.sanction_id,
            buyer_user_id: buyer().as_uuid(),
        }
    }

    pub(super) async fn scalar(&self, sql: &'static str) -> String {
        sqlx::query_scalar(sql).fetch_one(self.pool()).await.expect("读取标量")
    }

    pub(super) async fn buyer_status(&self) -> String {
        sqlx::query_scalar("SELECT status FROM public.users WHERE id = $1")
            .bind(buyer().as_uuid())
            .fetch_one(self.pool())
            .await
            .expect("买家状态")
    }

    pub(super) async fn blacklist_history(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM public.user_blacklist_history WHERE user_id = $1")
            .bind(buyer().as_uuid())
            .fetch_one(self.pool())
            .await
            .expect("黑名单历史数")
    }

    /// 买家是否被新购阻断：有待执行处罚，或账号已不是 active。
    pub(super) async fn purchase_blocked(&self, n: u128) -> bool {
        let pending = self
            .store
            .test_purchase_gate(command(n), buyer(), seller(), None)
            .await
            .expect("阻断读");
        pending || self.buyer_status().await != "active"
    }
}

/// 构成项的确定集合（升序拼接）。
async fn members(env: &Env) -> String {
    sqlx::query_scalar(
        "SELECT string_agg(return_fact_id::text, ',' ORDER BY return_fact_id)
         FROM public.consignment_sanction_members WHERE sanction_id = $1",
    )
    .bind(env.stock.sanction_id)
    .fetch_one(env.pool())
    .await
    .expect("构成项")
}

/// 处罚前已成立的订单与挂单、代管事实的整行快照。
async fn orders_snapshot(env: &Env) -> String {
    env.scalar(
        "SELECT coalesce(string_agg(to_jsonb(sale)::text, '|' ORDER BY sale.contract_id::text), '')
                || '#' || coalesce((SELECT string_agg(to_jsonb(listing)::text, '|' ORDER BY listing.id::text)
                                    FROM public.consignment_listings AS listing), '')
                || '#' || coalesce((SELECT string_agg(to_jsonb(position)::text, '|' ORDER BY position.id::text)
                                    FROM public.consignment_custody_positions AS position), '')
         FROM public.consignment_sales AS sale",
    )
    .await
}

async fn insert_buyer_session(env: &Env) {
    sqlx::query(
        "INSERT INTO public.public_sessions
            (token_hash, user_id, created_at, last_activity_at, expires_at, absolute_expires_at)
         VALUES (decode(repeat('5e', 32), 'hex'), $1, now() - interval '1 hour',
                 now() - interval '1 hour', now() + interval '1 day', now() + interval '2 days')",
    )
    .bind(buyer().as_uuid())
    .execute(env.pool())
    .await
    .expect("买家会话");
}

fn applied_command(outcome: SanctionWorkOutcome) -> Option<Uuid> {
    match outcome {
        SanctionWorkOutcome::Applied { account_command_id } => Some(account_command_id),
        _ => None,
    }
}

/// 处罚固定后的任意时刻接管：窗口不再重数，处罚恰一项、账号效果恰一次、构成项仍为原三份、
/// 新购全程受阻、第三封本金回补仍为 0；并发、重放与重启都不改变这些结果。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_handoff_applies_original_sanction_exactly_once() {
    let env = Env::isekai().await;
    let members_before = members(&env).await;
    assert_eq!(members_before.split(',').count(), 3, "构成项是原三份");
    let orders_before = orders_snapshot(&env).await;
    insert_buyer_session(&env).await;
    assert!(env.purchase_blocked(900).await, "执行前已由待执行处罚阻断");
    assert_eq!(env.store.open_sanction_work(10).await.expect("待执行工作"), vec![env.work()]);

    // 三个并发领取：买家锁串行化，恰一个落效果，其余只重放。
    let (a, b, c) = tokio::join!(
        env.store.execute_sanction_work(env.work()),
        env.store.execute_sanction_work(env.work()),
        env.store.execute_sanction_work(env.work()),
    );
    let outcomes = [a.expect("并发执行"), b.expect("并发执行"), c.expect("并发执行")];
    let applied: Vec<Uuid> = outcomes.iter().copied().filter_map(applied_command).collect();
    assert_eq!(applied.len(), 1, "账号效果恰一次：{outcomes:?}");
    assert_eq!(
        outcomes.iter().filter(|outcome| **outcome == SanctionWorkOutcome::Replayed).count(),
        2
    );

    // 重放与重启（新连接池）：原结论，无新写入。
    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("重放"),
        SanctionWorkOutcome::Replayed
    );
    let restarted = env.database.store().await.expect("重启后的运行账号连接");
    assert_eq!(
        restarted.execute_sanction_work(env.work()).await.expect("重启后重放"),
        SanctionWorkOutcome::Replayed
    );
    assert!(restarted.open_sanction_work(10).await.expect("待执行工作").is_empty());

    assert_eq!(count(env.pool(), "consignment_sanctions").await, 1, "处罚累计恰一项");
    assert_eq!(env.scalar("SELECT state FROM public.consignment_sanctions").await, "applied");
    assert_eq!(members(&env).await, members_before, "构成项仍为原三份");
    assert_eq!(count(env.pool(), "consignment_sanction_effects").await, 1);
    assert_eq!(count(env.pool(), "consignment_sanction_work_results").await, 1);
    let effect_command: Uuid =
        sqlx::query_scalar("SELECT account_command_id FROM public.consignment_sanction_effects")
            .fetch_one(env.pool())
            .await
            .expect("效果的账号命令");
    assert_eq!(effect_command, applied[0]);
    assert_eq!(env.blacklist_history().await, 1, "黑名单历史恰一条");
    // 账本里恰一条 apply_sanction 回执，与效果同源；重放 / 重启 / 并发没有多写。
    assert_eq!(
        env.scalar(
            "SELECT count(*)::text || ':' || min(result_body->>'effect_state')
                    || ':' || min(result_body->>'account_command_id')
             FROM public.consignment_commands WHERE operation = 'apply_sanction'"
        )
        .await,
        format!("1:applied:{effect_command}")
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE action = 'user.blacklist' AND metadata->>'command_id' = $1",
    )
    .bind(effect_command.to_string())
    .fetch_one(env.pool())
    .await
    .expect("审计数");
    assert_eq!(audits, 1);
    // 账号副作用复用 Users 的既有路径：公开会话被撤销。
    let live_sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.public_sessions WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(buyer().as_uuid())
    .fetch_one(env.pool())
    .await
    .expect("有效会话数");
    assert_eq!(live_sessions, 0);

    assert!(env.purchase_blocked(901).await, "执行后仍受阻（账号已拉黑）");
    // 拉黑只禁新购买：处罚前已成立的订单、挂单与代管事实逐行不变。
    assert_eq!(orders_snapshot(&env).await, orders_before);
    // 第三封的本金回补仍为 0：处罚执行不触碰资金。
    assert_eq!(count(env.pool(), "consignment_funding_returns").await, 0);
    assert_eq!(count(env.pool(), "consignment_return_consumptions").await, 0);
}

/// 账号命令已提交、效果尚未写入时进程中断：重入直接登记效果，不再调用 Users，黑名单历史不增。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_handoff_resumes_after_account_command_committed() {
    let env = Env::isekai().await;
    let t0: Timestamp = sqlx::query_scalar("SELECT t0 FROM public.consignment_sanctions")
        .fetch_one(env.pool())
        .await
        .expect("处罚判定时刻");
    let account_command = consignment_sanction_blacklist_command(&ConsignmentSanctionAccountWork {
        sanction_id: env.stock.sanction_id,
        work_id: env.stock.account_work_id,
        buyer_user_id: buyer().as_uuid(),
        rule: SanctionRule::IsekaiUnpaidCarrierReturns,
        occurred_at: t0,
    })
    .expect("账号命令");
    let committed = account_command.command_id;
    env.store.ensure_buyer_blacklisted(account_command).await.expect("账号命令先提交");
    sqlx::query(
        "UPDATE public.consignment_account_work SET state = 'claimed', version = version + 1
         WHERE id = $1",
    )
    .bind(env.stock.account_work_id)
    .execute(env.pool())
    .await
    .expect("工作停在 claimed");

    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("重入"),
        SanctionWorkOutcome::Applied { account_command_id: committed }
    );
    assert_eq!(env.blacklist_history().await, 1, "没有第二次账号效果");
    assert_eq!(count(env.pool(), "consignment_sanction_effects").await, 1);
    assert_eq!(env.scalar("SELECT state FROM public.consignment_sanctions").await, "applied");
}

/// 反例：工作与买家不符时失败关闭，不写任何效果。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_handoff_rejects_mismatched_buyer() {
    let env = Env::isekai().await;
    let forged = SanctionWorkRef { buyer_user_id: seller().as_uuid(), ..env.work() };
    assert!(env.store.execute_sanction_work(forged).await.is_err());
    assert_eq!(count(env.pool(), "consignment_sanction_effects").await, 0);
    assert_eq!(env.buyer_status().await, "active");
}

/// 运行循环直到它完成第一轮，返回每轮是否记了错误；随后发 drain 并等待退出。
async fn run_loop_one_round(env: &Env) -> Vec<bool> {
    let runtime = Arc::new(SanctionExecutionRuntime {
        store: Arc::clone(&env.store) as Arc<dyn SanctionWorkStore>,
        wakeup: Arc::new(Notify::new()),
    });
    let (drain, stopped) = watch::channel(false);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&seen);
    let handle = tokio::spawn(async move {
        run_sanction_execution(runtime, stopped, move |result, _duration| {
            observed.lock().expect("观察记录").push(matches!(result, WorkerResult::Error));
        })
        .await;
    });
    for _ in 0..200 {
        if !seen.lock().expect("观察记录").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drain.send(true).expect("发送 drain");
    handle.await.expect("循环退出");
    let rounds = seen.lock().expect("观察记录");
    rounds.clone()
}

/// 处罚固定两天之后才由第 7 期接管：此时按当天窗口只剩两份，执行不读时钟做判定、不重数窗口，效果只在原处罚上
/// 落一次，构成项仍为原三份；此后补齐精确 Take 并提交资金消费，才结清本金 1，处罚总数仍为一。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_handoff_two_days_after_t0_keeps_original_members() {
    let env = Env::isekai().await;
    let members_before = members(&env).await;
    let (t0, takeover): (Timestamp, Timestamp) =
        sqlx::query_as("SELECT t0, t0 + interval '2 days' FROM public.consignment_sanctions")
            .fetch_one(env.pool())
            .await
            .expect("t0 与接管时刻");
    assert!(env.purchase_blocked(910).await, "接管前新购受阻");
    // 按接管当天的 (takeover−90 天, takeover] 窗口，最早一份已出窗，只剩两份。
    let in_takeover_window: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_sanction_members AS member
         JOIN public.consignment_return_facts AS fact ON fact.return_fact_id = member.return_fact_id
         WHERE member.sanction_id = $1 AND fact.returned_at > $2 - interval '90 days'
           AND fact.returned_at <= $2",
    )
    .bind(env.stock.sanction_id)
    .bind(takeover)
    .fetch_one(env.pool())
    .await
    .expect("接管当天窗内构成项");
    assert_eq!(in_takeover_window, 2);

    let outcome =
        env.store.test_execute_sanction_work_at(env.work(), takeover).await.expect("t0+2 天接管");
    assert!(matches!(outcome, SanctionWorkOutcome::Applied { .. }));
    let applied_at: Timestamp =
        sqlx::query_scalar("SELECT applied_at FROM public.consignment_sanction_effects")
            .fetch_one(env.pool())
            .await
            .expect("效果时刻");
    assert_eq!(applied_at, takeover, "效果落在接管时刻，且晚于 t0 两天：{t0:?}");
    assert_eq!(count(env.pool(), "consignment_sanctions").await, 1);
    assert_eq!(members(&env).await, members_before, "构成项仍为原三份");
    assert_eq!(count(env.pool(), "consignment_sanction_effects").await, 1);
    assert!(env.purchase_blocked(911).await);
    assert_eq!(count(env.pool(), "consignment_funding_returns").await, 0);
    // 重放不再改变任何东西。
    assert_eq!(
        env.store.test_execute_sanction_work_at(env.work(), takeover).await.expect("重放"),
        SanctionWorkOutcome::Replayed
    );
    assert_eq!(env.blacklist_history().await, 1);

    // 第三封本金在接管前后都未回补；补齐精确 Take 与资金消费后才结清 1，处罚、构成项与效果都不变。
    assert_eq!(principal_effects(env.pool()).await, (0, 0, 0, 1));
    let isekai = env.isekai.as_ref().expect("isekai 存量");
    isekai.consume_third_take(&env.store, 0x61).await.expect("精确 Take 与资金消费");
    assert_eq!(principal_effects(env.pool()).await, (1, 1, 1, 0));
    assert_eq!(own(env.pool()).await, OWN);
    assert_eq!(count(env.pool(), "consignment_sanctions").await, 1);
    assert_eq!(members(&env).await, members_before);
    assert_eq!(count(env.pool(), "consignment_sanction_effects").await, 1);
    assert!(env.purchase_blocked(912).await);
}

/// 买家因别的途径已被禁（这里是另一项无关处罚的拉黑命令）：效果记“已禁、仅记录”，不引用那条命令
/// （反例：效果的账号命令必须为空），不追加黑名单历史，账本回执带 already_blacklisted。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_handoff_records_already_blacklisted_without_adopting_unrelated_command()
 {
    let env = Env::isekai().await;
    let t0: Timestamp = sqlx::query_scalar("SELECT t0 FROM public.consignment_sanctions")
        .fetch_one(env.pool())
        .await
        .expect("处罚判定时刻");
    let unrelated = consignment_sanction_blacklist_command(&ConsignmentSanctionAccountWork {
        sanction_id: Uuid::from_u128(0x7b20_0000_0000_4000_8000_0000_0000_0001),
        work_id: Uuid::from_u128(0x7b20_0000_0000_4000_8000_0000_0000_0002),
        buyer_user_id: buyer().as_uuid(),
        rule: SanctionRule::MainCodReturns,
        occurred_at: t0,
    })
    .expect("无关命令");
    env.store.ensure_buyer_blacklisted(unrelated).await.expect("无关拉黑命令先提交");

    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("执行"),
        SanctionWorkOutcome::AlreadyBlacklisted
    );
    assert_eq!(
        env.scalar("SELECT state || ':' || (account_command_id IS NULL)::text FROM public.consignment_sanction_effects")
            .await,
        "already_blacklisted:true",
        "不引用任何拉黑命令"
    );
    assert_eq!(env.scalar("SELECT state FROM public.consignment_sanctions").await, "applied");
    assert_eq!(env.blacklist_history().await, 1, "没有为本处罚追加任何账号写入");
    assert_eq!(
        env.scalar(
            "SELECT result_body->>'effect_state' FROM public.consignment_commands
             WHERE operation = 'apply_sanction'"
        )
        .await,
        "already_blacklisted"
    );
    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("重放"),
        SanctionWorkOutcome::Replayed
    );
}

/// 反例：工作已 completed 却没有效果回执（不可能的状态）：被列出，执行器失败关闭且无写入，循环记错误。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_handoff_completed_work_without_effect_fails_closed()
{
    let env = Env::isekai().await;
    for (from, to) in [("pending", "claimed"), ("claimed", "completed")] {
        sqlx::query(
            "UPDATE public.consignment_account_work SET state = $3, version = version + 1
             WHERE id = $1 AND state = $2",
        )
        .bind(env.stock.account_work_id)
        .bind(from)
        .bind(to)
        .execute(env.pool())
        .await
        .expect("造不可能的存量");
    }
    assert_eq!(env.store.open_sanction_work(10).await.expect("待执行工作"), vec![env.work()]);
    assert_eq!(
        env.store.execute_sanction_work(env.work()).await.expect("执行"),
        SanctionWorkOutcome::Inconsistent
    );
    assert_eq!(run_loop_one_round(&env).await, vec![true], "循环把它记为错误");
    assert_eq!(count(env.pool(), "consignment_sanction_effects").await, 0);
    assert_eq!(env.buyer_status().await, "active");
    assert_eq!(env.blacklist_history().await, 0);
}

/// 进程在账号命令提交后中断留下的 claimed 工作：被列出，并由运行循环收口为 applied。
#[tokio::test]
async fn postgres_consignment_recovery_sanction_handoff_loop_closes_claimed_work() {
    let env = Env::isekai().await;
    sqlx::query(
        "UPDATE public.consignment_account_work SET state = 'claimed', version = version + 1
         WHERE id = $1",
    )
    .bind(env.stock.account_work_id)
    .execute(env.pool())
    .await
    .expect("工作停在 claimed");
    assert_eq!(env.store.open_sanction_work(10).await.expect("待执行工作"), vec![env.work()]);

    assert_eq!(run_loop_one_round(&env).await.first(), Some(&false), "第一轮成功");
    assert_eq!(env.scalar("SELECT state FROM public.consignment_sanctions").await, "applied");
    assert_eq!(count(env.pool(), "consignment_sanction_effects").await, 1);
    assert!(env.store.open_sanction_work(10).await.expect("待执行工作").is_empty());
}
