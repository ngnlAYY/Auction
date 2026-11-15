//! 回归：投递唤醒订阅断开后只记录一次断开、按指数退避重连，恢复时记录一次；排空立即返回。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auction_application::core::ApplicationFuture;
use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation, DeliverySendResult,
    DeliveryTransportError, DeliveryWakeup, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::settlement::SettlementInternalEvidenceSnapshot;
use chrono::{DateTime, Utc};
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

use super::wakeup::run;
use crate::worker::runtime::test_fixture::unavailable;

/// 原始连接日志的 target；诊断窗口的分类日志使用独立 target，不计入断言。
const WAKEUP_TARGET: &str = "auction_jobs::worker::runtime::delivery::wakeup";

/// 订阅保持指定时长后断开；`None` 表示一直保持且不收到提示。
struct ScriptedWakeup(Option<Duration>);

impl DeliveryWakeup for ScriptedWakeup {
    fn wait(&mut self) -> ApplicationFuture<'_, ()> {
        let hold = self.0;
        Box::pin(async move {
            let Some(hold) = hold else {
                return std::future::pending().await;
            };
            tokio::time::sleep(hold).await;
            Err(ApplicationError::Repository(RepositoryError::Failed("订阅连接已断开".to_owned())))
        })
    }
}

/// 一次订阅请求的结果。
enum Attempt {
    Fail,
    Hold(Option<Duration>),
}

/// 按脚本应答订阅并记录尝试时刻；脚本用尽后持续失败。投递端口不参与本组用例。
struct ScriptedStore {
    script: Mutex<VecDeque<Attempt>>,
    attempts: Mutex<Vec<Instant>>,
}

impl ScriptedStore {
    fn new(script: Vec<Attempt>) -> Arc<Self> {
        Arc::new(Self { script: Mutex::new(script.into()), attempts: Mutex::new(Vec::new()) })
    }

    /// 相对首次尝试的秒数。
    fn attempt_offsets(&self) -> Vec<u64> {
        let attempts = self.attempts.lock().unwrap();
        let first = attempts[0];
        attempts.iter().map(|attempt| attempt.duration_since(first).as_secs()).collect()
    }
}

impl DeliveryWorkStore for ScriptedStore {
    fn subscribe_wakeups(&self) -> ApplicationFuture<'_, Box<dyn DeliveryWakeup>> {
        self.attempts.lock().unwrap().push(Instant::now());
        let attempt = self.script.lock().unwrap().pop_front().unwrap_or(Attempt::Fail);
        Box::pin(async move {
            match attempt {
                Attempt::Fail => Err(ApplicationError::Repository(RepositoryError::Failed(
                    "订阅连接不可用".to_owned(),
                ))),
                Attempt::Hold(hold) => {
                    Ok(Box::new(ScriptedWakeup(hold)) as Box<dyn DeliveryWakeup>)
                }
            }
        })
    }

    fn claim_due(
        &self,
        _worker_id: String,
        _now: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
        _sending_grace: Duration,
        _exclusions: DeliveryClaimExclusions,
        _limit: u16,
    ) -> ApplicationFuture<'_, Vec<DeliveryWorkClaim>> {
        unavailable()
    }

    fn begin_claim_cycle(
        &self,
        _now: DateTime<Utc>,
        _sending_grace: Duration,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }

    fn start_sending_authorized<'a>(
        &'a self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
        _evidence: Option<&'a SettlementInternalEvidenceSnapshot>,
        _expectation: Option<&'a DeliveryPreparedExpectation>,
    ) -> ApplicationFuture<'a, DeliveryFenceOutcome> {
        unavailable()
    }

    fn start_sending_authorized_until<'a>(
        &'a self,
        _claim: DeliveryWorkClaim,
        _started_at: DateTime<Utc>,
        _evidence: &'a SettlementInternalEvidenceSnapshot,
        _deadline: std::time::Instant,
        _expectation: Option<&'a DeliveryPreparedExpectation>,
    ) -> ApplicationFuture<'a, DeliveryFenceOutcome> {
        unavailable()
    }

    fn reject_preparation(
        &self,
        _claim: DeliveryWorkClaim,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }

    fn release(
        &self,
        _claim: DeliveryWorkClaim,
        _released_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }

    fn renew(
        &self,
        _claim: DeliveryWorkClaim,
        _renewed_at: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }

    fn succeed(
        &self,
        _claim: DeliveryWorkClaim,
        _result: DeliverySendResult,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }

    fn fail(
        &self,
        _claim: DeliveryWorkClaim,
        _error: DeliveryTransportError,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }
}

/// 只收集唤醒模块原始日志级别的最小订阅者；当前线程作用域，跨 await 的子任务同线程执行。
#[derive(Clone, Default)]
struct WakeupLogs(Arc<Mutex<Vec<Level>>>);

impl WakeupLogs {
    fn count(&self, level: Level) -> usize {
        self.0.lock().unwrap().iter().filter(|recorded| **recorded == level).count()
    }
}

/// 只确认事件携带消息字段；断言按级别区分断开与恢复。
struct MessagePresent(bool);

impl Visit for MessagePresent {
    fn record_debug(&mut self, field: &Field, _value: &dyn std::fmt::Debug) {
        self.0 |= field.name() == "message";
    }
}

impl Subscriber for WakeupLogs {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        if event.metadata().target() != WAKEUP_TARGET {
            return;
        }
        let mut message = MessagePresent(false);
        event.record(&mut message);
        if message.0 {
            self.0.lock().unwrap().push(*event.metadata().level());
        }
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[tokio::test(start_paused = true)]
async fn failures_back_off_exponentially_and_log_disconnect_once_until_restored() {
    let logs = WakeupLogs::default();
    let _logs = tracing::subscriber::set_default(logs.clone());
    let mut script = (0..6).map(|_| Attempt::Fail).collect::<Vec<_>>();
    script.push(Attempt::Hold(None));
    let store = ScriptedStore::new(script);
    let wakeup = Arc::new(Notify::new());
    let (drain, receiver) = watch::channel(false);
    let task = tokio::spawn(run(store.clone(), wakeup.clone(), receiver));

    tokio::time::sleep(Duration::from_secs(300)).await;

    // 5 → 10 → 20 → 40 → 60 封顶；修复前固定 5 秒且每次断开都有一条原始 WARN。
    assert_eq!(store.attempt_offsets(), vec![0, 5, 15, 35, 75, 135, 195]);
    assert_eq!(logs.count(Level::WARN), 1, "持续断开只记录一次");
    assert_eq!(logs.count(Level::INFO), 1, "恢复时记录一次");
    // 恢复后立即补扫一次，弥补断线窗口内丢失的提示。
    assert!(tokio::time::timeout(Duration::ZERO, wakeup.notified()).await.is_ok());

    drain.send_replace(true);
    tokio::time::timeout(Duration::from_millis(1), task).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn unstable_subscription_keeps_backing_off_until_it_stays_connected() {
    let logs = WakeupLogs::default();
    let _logs = tracing::subscriber::set_default(logs.clone());
    let store = ScriptedStore::new(vec![
        Attempt::Hold(Some(Duration::from_secs(1))),
        Attempt::Hold(Some(Duration::from_secs(1))),
        Attempt::Hold(Some(Duration::from_secs(120))),
        Attempt::Hold(None),
    ]);
    let (drain, receiver) = watch::channel(false);
    let task = tokio::spawn(run(store.clone(), Arc::new(Notify::new()), receiver));

    tokio::time::sleep(Duration::from_secs(300)).await;

    // 1 秒即断的订阅继续退避（5 → 10 秒）；稳定保持 120 秒后断开从 5 秒重新开始。
    assert_eq!(store.attempt_offsets(), vec![0, 6, 17, 142]);
    assert_eq!(logs.count(Level::WARN), 3);
    assert_eq!(logs.count(Level::INFO), 3);

    drain.send_replace(true);
    tokio::time::timeout(Duration::from_millis(1), task).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn drain_interrupts_reconnect_wait_immediately() {
    // 驱动 `run` 的用例都先装作用域订阅者：callsite 首次注册时若线程没有订阅者，可能与其他
    // 用例并发建立的订阅者竞争，被全局缓存为不关注，导致其他用例漏收日志。
    let _logs = tracing::subscriber::set_default(WakeupLogs::default());
    let store = ScriptedStore::new(Vec::new());
    let (drain, receiver) = watch::channel(false);
    let task = tokio::spawn(run(store.clone(), Arc::new(Notify::new()), receiver));

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain.send_replace(true);
    tokio::time::timeout(Duration::from_millis(1), task).await.unwrap().unwrap();
    assert_eq!(store.attempt_offsets(), vec![0], "排空期间不得再发起订阅");
}
