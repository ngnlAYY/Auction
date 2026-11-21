//! 结算退件 Take 与入站退回、恢复 Take 对同一邮件的并发互斥（PostgreSQL 18，运行账号，真实连接）：
//! 第三条连接先持有 world 行锁，两个事务依次在同一把锁上排队（以锁等待状态确认，不靠睡眠猜测
//! 时序），释放后按排队顺序先后提交。无论哪一方先提交，同一邮件只留下一项处置意图：先提交的
//! 一方生效，后到的入站退回或恢复 Take 被拒绝，后到的结算退件 Take 转为结果未知
//! （`mail_disposition_conflict`），不写入投递，动作保持没有投递身份的 pending。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use auction_application::core::{ApplicationError, RequestHash};
use auction_application::mail::{
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
};
use auction_application::settlement::{SettlementReturnProgress, SettlementReturnWorkStore};
use auction_domain::{CommandId, SettlementReturnAction, SettlementReturnCaseStatus};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::flows::{PAYOUT, handle, returned_payout, seed_spent_payout};
use crate::support;

/// 与结算退件 Take 竞争同一邮件的另一方。
#[derive(Debug, Clone, Copy)]
enum Competitor {
    /// 入站流程把邮件退回发件人。
    IntakeReturn,
    /// 恢复流程领取同一邮件。
    RecoveryTake,
}

impl Competitor {
    fn command(self, mid: &str) -> MailDeliveryCommand {
        let (origin, origin_id, operation, credits) = match self {
            Self::IntakeReturn => (
                MailDeliveryOrigin::MailIntake,
                format!("main:{mid}:return"),
                MailDeliveryOperation::Return,
                0,
            ),
            Self::RecoveryTake => (
                MailDeliveryOrigin::SettlementRecoveryAction,
                format!("recovery:{}", Uuid::new_v4()),
                MailDeliveryOperation::Take,
                PAYOUT,
            ),
        };
        MailDeliveryCommand {
            command_id: CommandId::from_uuid(Uuid::new_v4()),
            request_hash: RequestHash::new([0x6c; 32]),
            origin,
            origin_id,
            operation,
            world_id: "main".to_owned(),
            external_mail_id: mid.to_owned(),
            recipient: "MoogleMail".to_owned(),
            template: None,
            result_code: None,
            user_key_user_id: None,
            credits,
            cod: 0,
            attachment_ids: Vec::new(),
            notification_body: None,
        }
    }

    async fn run(self, store: &DatabaseStore, mid: &str) -> Result<(), ApplicationError> {
        match self {
            Self::IntakeReturn => store.enqueue(self.command(mid)).await.map(drop),
            Self::RecoveryTake => store.prepare_recovery(self.command(mid)).await.map(drop),
        }
    }

    fn origin(self) -> &'static str {
        match self {
            Self::IntakeReturn => "mail_intake",
            Self::RecoveryTake => "settlement_recovery_action",
        }
    }
}

/// 等到恰有 `count` 个会话被本轮指定的行锁持有者阻塞，超时失败。
async fn wait_for_lock_waiters(pool: &PgPool, count: i64, blocker_pid: i32) {
    let deadline = Instant::now() + StdDuration::from_secs(10);
    loop {
        let waiting = crate::support::lock_waiters::count_blocked_by(pool, blocker_pid)
            .await
            .expect("读取锁等待");
        if waiting >= count {
            return;
        }
        assert!(Instant::now() < deadline, "等待第 {count} 个目标锁等待者超时");
        tokio::time::sleep(StdDuration::from_millis(10)).await;
    }
}

/// 同一邮件上的各来源处置意图。
async fn intents(pool: &PgPool, mid: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT origin_kind FROM public.deliveries
         WHERE payload_world = 'main' AND payload->>'external_mail_id' = $1
         ORDER BY origin_kind",
    )
    .bind(mid)
    .fetch_all(pool)
    .await
    .expect("读取处置意图")
}

/// 一轮竞争：`return_first` 为真时结算退件 Take 先排队、先提交。
async fn race(
    store: &Arc<DatabaseStore>,
    competitor: Competitor,
    return_first: bool,
    sequence: i64,
) {
    let pool = store.postgres_pool();
    let mid = format!("{}", 999_000 + sequence % 1_000);
    let returned = returned_payout(store, sequence, &mid).await;
    seed_spent_payout(pool, &returned).await;
    assert_eq!(
        handle(store, returned.case, 1, SettlementReturnAction::ReclaimAndHold).await,
        SettlementReturnCaseStatus::TakePending
    );

    let mut holder = pool.acquire().await.expect("取得持锁连接");
    sqlx::query("BEGIN").execute(&mut *holder).await.expect("开启持锁事务");
    sqlx::query("SELECT id FROM public.worlds WHERE id = 'main' FOR UPDATE")
        .execute(&mut *holder)
        .await
        .expect("持有 world 行锁");
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .expect("读取 world 锁持有会话");

    let activate = {
        let store = Arc::clone(store);
        move || {
            tokio::spawn(async move {
                store.activate_due_return_take(Timestamp::now(), super::take_worlds()).await
            })
        }
    };
    let compete = {
        let store = Arc::clone(store);
        let mid = mid.clone();
        move || tokio::spawn(async move { competitor.run(&store, &mid).await })
    };
    let (activation, competition) = if return_first {
        let activation = activate();
        wait_for_lock_waiters(pool, 1, blocker_pid).await;
        let competition = compete();
        wait_for_lock_waiters(pool, 2, blocker_pid).await;
        (activation, competition)
    } else {
        let competition = compete();
        wait_for_lock_waiters(pool, 1, blocker_pid).await;
        let activation = activate();
        wait_for_lock_waiters(pool, 2, blocker_pid).await;
        (activation, competition)
    };
    sqlx::query("COMMIT").execute(&mut *holder).await.expect("释放 world 行锁");
    drop(holder);
    let activation: Option<SettlementReturnProgress> =
        activation.await.expect("激活任务不得 panic").expect("激活事务必须确定提交");
    let competition = competition.await.expect("竞争任务不得 panic");
    let progress = activation.expect("待执行的 Take 必须被处理");
    let (status, action_state, failure): (String, String, Option<String>) = sqlx::query_as(
        "SELECT c.status, a.execution_state, a.failure_code
         FROM public.settlement_return_cases c
         JOIN public.settlement_return_actions a ON a.return_mail_id = c.return_mail_id
         WHERE c.return_mail_id = $1",
    )
    .bind(returned.case)
    .fetch_one(pool)
    .await
    .expect("读取案件与动作");
    let label = format!("{competitor:?} return_first={return_first}");
    if return_first {
        assert!(competition.is_err(), "后到的另一方必须被拒绝（{label}）");
        assert_eq!(progress.status, SettlementReturnCaseStatus::TakePending, "{label}");
        assert_eq!(
            (status.as_str(), action_state.as_str()),
            ("take_pending", "enqueued"),
            "{label}"
        );
        assert_eq!(intents(pool, &mid).await, ["settlement_return_action"], "{label}");
    } else {
        competition.expect("先到的另一方必须成功");
        assert_eq!(progress.status, SettlementReturnCaseStatus::TakeUnknown, "{label}");
        assert_eq!(
            (status.as_str(), action_state.as_str(), failure.as_deref()),
            ("take_unknown", "pending", None),
            "{label}"
        );
        let (kind, details): (String, serde_json::Value) = sqlx::query_as(
            "SELECT event_kind, details FROM public.settlement_return_events
             WHERE return_mail_id = $1 ORDER BY case_version DESC LIMIT 1",
        )
        .bind(returned.case)
        .fetch_one(pool)
        .await
        .expect("读取最新事件");
        assert_eq!(
            (kind.as_str(), details["failure_code"].as_str()),
            ("take_unknown", Some("mail_disposition_conflict")),
            "冲突原因记在案件事件里（{label}）"
        );
        assert_eq!(
            intents(pool, &mid).await,
            [competitor.origin()],
            "后到的 Take 不写入投递（{label}）"
        );
    }
}

/// 两个连接并发处置同一邮件，两种提交顺序 × 入站退回与恢复 Take：只有先提交的一方生效。
#[tokio::test]
async fn postgres_return_take_conflicts_with_intake_and_recovery() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = Arc::new(database.store().await.expect("以运行账号连接"));
    let mut sequence = 7_640_001;
    for competitor in [Competitor::IntakeReturn, Competitor::RecoveryTake] {
        for return_first in [true, false] {
            race(&store, competitor, return_first, sequence).await;
            sequence += 1;
        }
    }
    store.postgres_pool().close().await;
}
