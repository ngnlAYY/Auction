//! 回归：收件旧路径与独立维护任务按同一顺序（先配额许可、后世界维护锁）取得资源。
//!
//! 场景是隔离开关切换窗口：收件按旧路径排队等许可，维护任务按隔离模式到点。配额只有 1
//! 时，顺序相反会让“持许可等锁”与“持锁等许可”互相等待，并堵住全部批量循环。

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use auction_application::admin::AdminMailPollState;
use auction_application::auction::ApplicationFuture;
use auction_application::mail::{MailPollBatch, MailPollClaim, MailPollCompletion, MailPollStore};
use auction_application::runtime_switches::RuntimeSwitches;
use auction_application::scheduling::RecurringSchedule;
use auction_observability::{WorkerLoop, WorkerResult};
use chrono::{DateTime, Utc};

use super::maintenance::run_loop as run_maintenance_loop;
use super::poll::cycle::run_cycle;
use super::poll::{
    MailPollFetchRequest, MailPollGateway, MailPollGatewayError, MailPollPage, MailPollRuntime,
    MailPollRuntimeBindings, MailPollRuntimeConfig, MailPollSchedules,
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

/// 邮箱始终没有到期状态；领取之外的端口失败关闭。
struct IdleMailbox;

impl MailPollStore for IdleMailbox {
    fn lookup_completion(
        &self,
        _mailbox_key: String,
        _world_id: String,
        _external_mail_id: String,
        _received_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollCompletion> {
        unavailable()
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
        _mailbox_key: String,
        _worker_id: String,
        _now: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
        _limit: u16,
        _force: bool,
    ) -> ApplicationFuture<'_, Vec<MailPollClaim>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn renew(
        &self,
        _claim: MailPollClaim,
        _now: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollClaim> {
        unavailable()
    }

    fn commit(
        &self,
        _claim: MailPollClaim,
        _batch: MailPollBatch,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        unavailable()
    }

    fn fail(
        &self,
        _claim: MailPollClaim,
        _retry_at: DateTime<Utc>,
        _failure_code: String,
        _safe_detail: String,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        unavailable()
    }
}

impl MailPollGateway for IdleMailbox {
    fn fetch(
        &self,
        _request: MailPollFetchRequest,
    ) -> Pin<Box<dyn Future<Output = Result<MailPollPage, MailPollGatewayError>> + Send + '_>> {
        Box::pin(async {
            Err(MailPollGatewayError {
                failure_code: "unexpected_fetch".to_owned(),
                safe_detail: "测试邮箱没有可领取状态".to_owned(),
                temporary: false,
            })
        })
    }
}

fn mail_runtime() -> MailPollRuntime {
    let schedule: Arc<dyn RecurringSchedule> = Arc::new(EveryMinute);
    let mailbox = Arc::new(IdleMailbox);
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
    let runtime = mail_runtime();
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
