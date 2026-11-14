//! 回归：邮箱收件循环的加锁顺序、续租围栏与批次提交边界。
//!
//! - 收件旧路径与独立维护任务按同一顺序（先配额许可、后世界维护锁）取得资源。场景是隔离
//!   开关切换窗口：收件按旧路径排队等许可，维护任务按隔离模式到点。配额只有 1 时，顺序相反
//!   会让“持许可等锁”与“持锁等许可”互相等待，并堵住全部批量循环。
//! - 续租被拒绝即取消陈旧处理，不提交批次也不改写失败退避；批次预算耗尽或后缀失败时只提交
//!   已完成的连续前缀，游标停在最后一封已完成邮件，绝不使用整页游标。

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::admin::AdminMailPollState;
use auction_application::auction::ApplicationFuture;
use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::mail::{MailPollBatch, MailPollClaim, MailPollCompletion, MailPollStore};
use auction_application::runtime_switches::RuntimeSwitches;
use auction_application::scheduling::RecurringSchedule;
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::maintenance::run_loop as run_maintenance_loop;
use super::poll::cycle::run_cycle;
use super::poll::{
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime, MailPollRuntimeBindings, MailPollRuntimeConfig,
    MailPollSchedules,
};
use crate::worker::PersistentWorker;
use crate::worker::runtime::WorkerConnectionQuota;
use crate::worker::runtime::test_fixture::{
    PausedClock, RecordingMetrics, base_time, unavailable, worker,
};

/// 每分钟触发一次的日程；只决定唤醒时刻。
struct EveryMinute;

impl RecurringSchedule for EveryMinute {
    fn next_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        Some(now + chrono::Duration::minutes(1))
    }
}

/// 单邮箱替身：默认没有到期状态；装入一页邮件后只可领取一次，续租、完成事实核验与远程读取
/// 按脚本应答，并记录提交、前缀失败与失败退避。其余端口失败关闭。
#[derive(Default)]
struct ScriptedMailbox {
    /// 可领取一次的邮件页；为空表示邮箱始终没有到期状态。
    page: Vec<&'static str>,
    claimed: AtomicBool,
    /// 远程读取返回前保持的时长。
    fetch_hold: Duration,
    /// 续租被拒绝，例如租约已被其他 owner 接管。
    renew_rejected: bool,
    /// 完成事实核验一直挂起的邮件，用于耗尽批次预算。
    stalled_lookup: Option<&'static str>,
    /// 完成事实核验失败的邮件。
    failed_lookup: Option<&'static str>,
    fetches: AtomicUsize,
    lookups: Mutex<Vec<String>>,
    commits: Mutex<Vec<MailPollBatch>>,
    prefix_failures: Mutex<Vec<(MailPollBatch, String)>>,
    failures: Mutex<Vec<String>>,
}

impl ScriptedMailbox {
    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }

    fn lookups(&self) -> Vec<String> {
        self.lookups.lock().unwrap().clone()
    }

    fn commits(&self) -> Vec<MailPollBatch> {
        self.commits.lock().unwrap().clone()
    }

    fn prefix_failures(&self) -> Vec<(MailPollBatch, String)> {
        self.prefix_failures.lock().unwrap().clone()
    }

    fn failures(&self) -> Vec<String> {
        self.failures.lock().unwrap().clone()
    }
}

/// 存储确认后的邮箱状态；Worker 只关心调用是否成功。
fn committed_state(claim: &MailPollClaim) -> AdminMailPollState {
    AdminMailPollState {
        mailbox_key: claim.mailbox_key.clone(),
        cursor_value: claim.cursor_value.clone(),
        last_poll_at: base_time(),
        next_poll_at: base_time(),
        consecutive_failures: 0,
        paused: false,
        state_version: claim.state_version + 1,
        updated_at: base_time(),
    }
}

impl MailPollStore for ScriptedMailbox {
    fn lookup_completion(
        &self,
        _mailbox_key: String,
        _world_id: String,
        external_mail_id: String,
        _received_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollCompletion> {
        let stalled = self.stalled_lookup == Some(external_mail_id.as_str());
        let failed = self.failed_lookup == Some(external_mail_id.as_str());
        self.lookups.lock().unwrap().push(external_mail_id);
        Box::pin(async move {
            if stalled {
                std::future::pending::<()>().await;
            }
            if failed {
                return Err(ApplicationError::Repository(RepositoryError::Failed(
                    "完成事实查询失败".to_owned(),
                )));
            }
            Ok(MailPollCompletion::Missing)
        })
    }

    fn ensure_state(
        &self,
        _mailbox_key: String,
        _now: DateTime<Utc>,
        _scheduled_next_at: DateTime<Utc>,
        _retry_maximum: Duration,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        unavailable()
    }

    fn claim_due(
        &self,
        mailbox_key: String,
        worker_id: String,
        _now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        _limit: u16,
        _force: bool,
    ) -> ApplicationFuture<'_, Vec<MailPollClaim>> {
        let due = !self.page.is_empty() && !self.claimed.swap(true, Ordering::SeqCst);
        Box::pin(async move {
            if !due {
                return Ok(Vec::new());
            }
            Ok(vec![MailPollClaim {
                mailbox_key,
                worker_id,
                claim_token: Uuid::new_v4(),
                cursor_value: None,
                state_version: 1,
                paused: false,
                attempt_count: 1,
                lease_until,
            }])
        })
    }

    fn renew(
        &self,
        claim: MailPollClaim,
        _now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollClaim> {
        let rejected = self.renew_rejected;
        Box::pin(async move {
            if rejected {
                return Err(ApplicationError::Repository(RepositoryError::Failed(
                    "租约已被接管".to_owned(),
                )));
            }
            Ok(MailPollClaim { lease_until, ..claim })
        })
    }

    fn commit(
        &self,
        claim: MailPollClaim,
        batch: MailPollBatch,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        self.commits.lock().unwrap().push(batch);
        Box::pin(async move { Ok(committed_state(&claim)) })
    }

    fn fail_after_prefix(
        &self,
        claim: MailPollClaim,
        batch: MailPollBatch,
        _retry_at: DateTime<Utc>,
        failure_code: String,
        _safe_detail: String,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        self.prefix_failures.lock().unwrap().push((batch, failure_code));
        Box::pin(async move { Ok(committed_state(&claim)) })
    }

    fn fail(
        &self,
        claim: MailPollClaim,
        _retry_at: DateTime<Utc>,
        failure_code: String,
        _safe_detail: String,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        self.failures.lock().unwrap().push(failure_code);
        Box::pin(async move { Ok(committed_state(&claim)) })
    }
}

/// 只带安全摘要的已解析邮件；没有业务 envelope，未装配处理器时按待处理事实入站。
fn message(external_mail_id: &str) -> MailPollFetchedMessage {
    MailPollFetchedMessage {
        external_mail_id: external_mail_id.to_owned(),
        world_id: "main".to_owned(),
        received_at: base_time(),
        sender: Some("FixtureSender".to_owned()),
        subject: "auction".to_owned(),
        message_display: None,
        content_digest: [7; 32],
        attachment_count: 0,
        credits: 0,
        cod: 0,
        returnable: false,
        claimable: false,
        business: None,
    }
}

impl MailPollGateway for ScriptedMailbox {
    fn fetch(
        &self,
        request: MailPollFetchRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MailPollPage, MailPollGatewayError>> + Send + '_>> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(self.fetch_hold).await;
            Ok(MailPollPage {
                mailbox_key: request.mailbox_key,
                world_id: request.world_id,
                next_cursor: self
                    .page
                    .last()
                    .map(|external_mail_id| (*external_mail_id).to_owned()),
                messages: self
                    .page
                    .iter()
                    .map(|external_mail_id| message(external_mail_id))
                    .collect(),
            })
        })
    }
}

fn mail_runtime(mailbox: Arc<ScriptedMailbox>) -> MailPollRuntime {
    let schedule: Arc<dyn RecurringSchedule> = Arc::new(EveryMinute);
    MailPollRuntime::new(
        mailbox.clone(),
        mailbox,
        None,
        "main".to_owned(),
        "mailbox-main".to_owned(),
        MailPollRuntimeBindings {
            schedules: MailPollSchedules {
                receive: schedule.clone(),
                phased_receive: schedule.clone(),
                maintenance: schedule,
            },
            // 隔离基线为 true：维护任务按隔离模式到点，收件则模拟切换前已选定的旧路径。
            switches: RuntimeSwitches::default(),
        },
        MailPollRuntimeConfig::default(),
    )
    .unwrap()
}

/// 按隔离路径手动触发一轮收件；默认配置租约 60 秒、续租 10 秒、批次预算 600 秒。
async fn poll_once(mailbox: &Arc<ScriptedMailbox>) -> (Arc<RecordingMetrics>, Duration) {
    let metrics = Arc::new(RecordingMetrics::default());
    let worker = PersistentWorker {
        metrics: metrics.clone(),
        ..worker(Arc::new(PausedClock::new(base_time())))
    };
    let runtime = mail_runtime(mailbox.clone());
    let (_shutdown, mut receiver) = tokio::sync::watch::channel(false);
    let schedule: Arc<dyn RecurringSchedule> = Arc::new(EveryMinute);
    let started = tokio::time::Instant::now();
    run_cycle(&worker, &runtime, true, true, schedule, &mut receiver).await.unwrap();
    (metrics, started.elapsed())
}

fn intake_ids(batch: &MailPollBatch) -> Vec<&str> {
    batch.items.iter().map(|item| item.external_mail_id.as_str()).collect()
}

#[tokio::test(start_paused = true)]
async fn legacy_poll_and_isolated_maintenance_share_lock_order_on_single_permit() {
    let quota = Arc::new(WorkerConnectionQuota::new(1).unwrap());
    let metrics = Arc::new(RecordingMetrics::default());
    let clock = Arc::new(PausedClock::new(base_time()));
    let worker = Arc::new(PersistentWorker {
        bulk_quota: Some(quota.clone()),
        metrics: metrics.clone(),
        ..worker(clock)
    });
    let runtime = mail_runtime(Arc::new(ScriptedMailbox::default()));
    let (shutdown, receiver) = tokio::sync::watch::channel(false);

    // 第三方批量循环先占住唯一许可。
    let third_party = quota.acquire().await;
    // 收件已按旧路径选定本轮，排在许可队列首位。
    let poll = tokio::spawn({
        let worker = worker.clone();
        let runtime = runtime.clone();
        let mut receiver = receiver.clone();
        async move {
            let schedule: Arc<dyn RecurringSchedule> = Arc::new(EveryMinute);
            run_cycle(worker.as_ref(), &runtime, true, false, schedule, &mut receiver).await
        }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    // 维护任务按隔离模式运行，一分钟后到点。
    let maintenance =
        tokio::spawn(run_maintenance_loop(worker.clone(), runtime.clone(), receiver.clone()));
    tokio::time::sleep(Duration::from_secs(120)).await;

    // 维护到点后排队等许可期间不得已持有维护锁；修复前它先取锁，再等收件占住的许可。
    assert!(runtime.maintenance_lock.try_lock().is_ok(), "等待许可期间不得持有维护锁");

    drop(third_party);
    let polled = tokio::time::timeout(Duration::from_secs(30), poll).await;
    assert!(polled.is_ok(), "旧路径收件与维护任务不得互相等待");
    assert!(polled.unwrap().unwrap().is_ok());

    // 收件释放许可后，维护任务随即完成一轮。
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        metrics.iterations().contains(&(WorkerLoop::MailMaintenanceMain, WorkerResult::Success)),
        "维护任务应在收件之后完成一轮"
    );
    assert!(quota.try_acquire().is_some(), "两轮结束后许可必须归还");

    shutdown.send_replace(true);
    tokio::time::timeout(Duration::from_secs(30), maintenance).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn rejected_renewal_cancels_stale_processing_without_any_commit() {
    let mailbox = Arc::new(ScriptedMailbox {
        page: vec!["1001"],
        fetch_hold: Duration::from_secs(25),
        renew_rejected: true,
        ..ScriptedMailbox::default()
    });

    let (metrics, elapsed) = poll_once(&mailbox).await;

    // 首次续租（第 10 秒）被拒绝即丢弃远程读取；陈旧 owner 不得提交批次、前缀或失败退避，
    // 由数据库租约 fence 交给新 owner 恢复。
    assert_eq!(elapsed, Duration::from_secs(10));
    assert_eq!(mailbox.fetches(), 1);
    assert!(mailbox.commits().is_empty());
    assert!(mailbox.prefix_failures().is_empty());
    assert!(mailbox.failures().is_empty());
    assert_eq!(metrics.iterations(), vec![(WorkerLoop::MailMain, WorkerResult::Error)]);
}

#[tokio::test(start_paused = true)]
async fn exhausted_batch_budget_commits_only_completed_prefix() {
    let mailbox = Arc::new(ScriptedMailbox {
        page: vec!["1001", "1002", "1003"],
        stalled_lookup: Some("1002"),
        ..ScriptedMailbox::default()
    });

    let (metrics, elapsed) = poll_once(&mailbox).await;

    // 第二封的完成核验占满 600 秒批次预算，期间按续租维持 claim；到期后只提交第一封，
    // 未完成的后缀留给下一轮从持久待读队列恢复。批次截止取自真实单调时钟，只容许其流逝的
    // 少量误差。
    assert!(elapsed >= Duration::from_secs(590) && elapsed <= Duration::from_secs(600));
    let commits = mailbox.commits();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].next_cursor.as_deref(), Some("1001"), "游标不得越过未完成邮件");
    assert_eq!(intake_ids(&commits[0]), vec!["1001"]);
    assert_eq!(mailbox.lookups(), vec!["1001", "1002"]);
    assert!(mailbox.prefix_failures().is_empty());
    assert!(mailbox.failures().is_empty());
    assert_eq!(metrics.iterations(), vec![(WorkerLoop::MailMain, WorkerResult::Success)]);
}

#[tokio::test(start_paused = true)]
async fn failed_suffix_persists_completed_prefix_with_backoff_in_one_call() {
    let mailbox = Arc::new(ScriptedMailbox {
        page: vec!["1001", "1002", "1003"],
        failed_lookup: Some("1002"),
        ..ScriptedMailbox::default()
    });

    let (metrics, _elapsed) = poll_once(&mailbox).await;

    // 失败邮件之前的完成前缀与失败退避经同一 fenced 调用提交，绝不先 commit 再 fail。
    let prefix_failures = mailbox.prefix_failures();
    assert_eq!(prefix_failures.len(), 1);
    let (batch, failure_code) = &prefix_failures[0];
    assert_eq!(failure_code, "completion_lookup_failed");
    assert_eq!(batch.next_cursor.as_deref(), Some("1001"));
    assert_eq!(intake_ids(batch), vec!["1001"]);
    assert_eq!(mailbox.lookups(), vec!["1001", "1002"], "不得越过失败邮件继续处理");
    assert!(mailbox.commits().is_empty());
    assert!(mailbox.failures().is_empty());
    assert_eq!(metrics.iterations(), vec![(WorkerLoop::MailMain, WorkerResult::Error)]);
}
