//! 循环级回归测试的最小 Worker 装配：跟随 Tokio 暂停时间的时钟、失败关闭的惰性端口、
//! 已校验的热参数、可断言的指标记录与排空协议替身。只在 `cfg(test)` 编译，不进入运行时制品。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use auction_application::admin::{
    WorkerDeliveryHotConfig, WorkerForumHotConfig, WorkerHotConfig, WorkerMailHotConfig,
};
use auction_application::auction::{
    AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecycleResult,
};
use auction_application::core::ApplicationFuture;
use auction_application::core::diagnostics::SafeErrorKind;
use auction_application::core::{ApplicationError, Clock};
use auction_application::delivery::DeliverySendIntervals;
use auction_application::mail::MailSendClasses;
use auction_application::operations::{
    CriticalWriteKind, ExternalWriteActivity, ExternalWriteActivityFactory,
    ExternalWriteActivityPort, ExternalWriteLifecycle,
};
use auction_application::runtime_switches::RuntimeSwitches;
use auction_application::scheduling::{
    ScheduledRoundBootstrap, ScheduledRoundClaim, ScheduledRoundSnapshot, ScheduledRoundStore,
};
use auction_observability::{
    WorkerLoop, WorkerPanicOwner, WorkerRecorder, WorkerResult, WorkerRetryKind,
};
use chrono::{DateTime, Utc};

use super::{
    AuctionWorkerRuntimeConfig, DeliveryWorkerRuntimeConfig, ForumWorkerRuntimeConfig,
    FulfillmentWorkerRuntimeConfig, PersistentWorker, WorkerHotRuntime,
};
use crate::WorkerControlHandle;

/// 跟随 Tokio 时间推进的业务时钟；`start_paused` 测试里与 `tokio::time` 同步前进，
/// 让持久 `next_scan_at` 等时刻与进程内唤醒使用同一条时间线。
pub(crate) struct PausedClock {
    origin: tokio::time::Instant,
    base: DateTime<Utc>,
}

impl PausedClock {
    pub(crate) fn new(base: DateTime<Utc>) -> Self {
        Self { origin: tokio::time::Instant::now(), base }
    }
}

impl Clock for PausedClock {
    fn now(&self) -> DateTime<Utc> {
        let elapsed = tokio::time::Instant::now().saturating_duration_since(self.origin);
        self.base + chrono::Duration::from_std(elapsed).unwrap_or(chrono::Duration::MAX)
    }
}

/// 测试统一使用的固定起点；只作为时间线原点，不代表任何业务日程。
pub(crate) fn base_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-14T10:00:00Z")
        .unwrap_or_else(|error| panic!("固定测试时间应合法: {error:?}"))
        .with_timezone(&Utc)
}

/// 被调用即说明测试走进了未模拟的分支；统一以“能力不可用”失败关闭。
pub(crate) fn unavailable<'a, T: Send + 'a>() -> ApplicationFuture<'a, T> {
    Box::pin(async { Err(ApplicationError::CapabilityUnavailable("测试未装配该端口")) })
}

/// 周计划与生命周期端口的惰性替身；拍卖主循环之外的测试不会调用它们。
struct InertAuctionPorts;

impl ScheduledRoundStore for InertAuctionPorts {
    fn database_now(&self) -> ApplicationFuture<'_, DateTime<Utc>> {
        unavailable()
    }

    fn find_current_round(
        &self,
        _world_id: String,
        _local_week: chrono::NaiveDate,
    ) -> ApplicationFuture<'_, Option<ScheduledRoundSnapshot>> {
        unavailable()
    }

    fn find_slot_round(
        &self,
        _world_id: String,
        _slot_key: String,
    ) -> ApplicationFuture<'_, Option<ScheduledRoundSnapshot>> {
        unavailable()
    }

    fn ensure_current_round(
        &self,
        _bootstrap: ScheduledRoundBootstrap,
    ) -> ApplicationFuture<'_, ScheduledRoundSnapshot> {
        unavailable()
    }

    fn claim_due(
        &self,
        _worker_id: String,
        _now: DateTime<Utc>,
        _lease_until: DateTime<Utc>,
        _limit: u16,
    ) -> ApplicationFuture<'_, Vec<ScheduledRoundClaim>> {
        unavailable()
    }

    fn next_claimable_at(
        &self,
        _now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<DateTime<Utc>>> {
        unavailable()
    }

    fn complete(
        &self,
        _claim: ScheduledRoundClaim,
        _completed_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }

    fn fail(
        &self,
        _claim: ScheduledRoundClaim,
        _retry_at: DateTime<Utc>,
        _failure_code: String,
        _safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        unavailable()
    }
}

impl AuctionLifecycleCommandPort for InertAuctionPorts {
    fn allows_manual_early_open(&self) -> bool {
        false
    }

    fn allows_manual_early_settlement(&self) -> bool {
        false
    }

    fn execute(
        &self,
        _command: AuctionLifecycleCommand,
    ) -> ApplicationFuture<'_, AuctionLifecycleResult> {
        unavailable()
    }
}

/// 通过启动校验的热参数；论坛常规轮询 300 秒、抖动为零，唤醒时刻可精确断言。
pub(crate) fn hot_config() -> WorkerHotConfig {
    WorkerHotConfig {
        mail: WorkerMailHotConfig {
            batch_size: 10,
            jitter_seconds: 0,
            fetch_attempts: 3,
            fetch_timeout_seconds: 30,
            process_attempts: 3,
            item_timeout_seconds: 30,
            batch_timeout_seconds: 600,
            retry_base_seconds: 1,
            retry_maximum_seconds: 30,
            account_reply_window_seconds: 86_400,
            explanation_mail_window_seconds: 86_400,
            explanation_mail_limit: 10,
            send_classes: MailSendClasses::default(),
            send_intervals: DeliverySendIntervals::default(),
        },
        forum: WorkerForumHotConfig {
            batch_size: 10,
            poll_interval_seconds: 300,
            jitter_seconds: 0,
            item_timeout_seconds: 60,
        },
        delivery: WorkerDeliveryHotConfig {
            batch_limit: 10,
            poll_interval_seconds: 60,
            jitter_seconds: 0,
            request_timeout_seconds: 30,
            finalize_timeout_seconds: 5,
        },
    }
}

/// 未注入任何可选 owner 的 Worker；测试按需用结构体更新语法替换字段。
pub(crate) fn worker(clock: Arc<PausedClock>) -> PersistentWorker<PausedClock> {
    let ports = Arc::new(InertAuctionPorts);
    PersistentWorker {
        schedule: ports.clone(),
        worlds: None,
        lifecycle: ports,
        slow_proxy_due: None,
        slow_proxy_commands: None,
        auction: AuctionWorkerRuntimeConfig::default(),
        schedule_runtime: None,
        settlement_store: None,
        settlement_commands: None,
        fulfillment_store: None,
        fulfillment_commands: None,
        fulfillment: FulfillmentWorkerRuntimeConfig::default(),
        recovery_health: None,
        asset_reconciliation: None,
        recovery_store: None,
        recovery_resolution: None,
        recovery_delivery: None,
        recovery_blacklister: None,
        recovery: FulfillmentWorkerRuntimeConfig::default(),
        delivery_store: None,
        delivery_transport: None,
        delivery: DeliveryWorkerRuntimeConfig::default(),
        hot: WorkerHotRuntime::new(hot_config()),
        mail_polls: Vec::new(),
        forum: ForumWorkerRuntimeConfig {
            poll_interval: std::time::Duration::from_secs(300),
            jitter: std::time::Duration::ZERO,
            lease_duration: std::time::Duration::from_secs(300),
        },
        forum_sync: None,
        forum_reply: None,
        operations_alert: None,
        player_notifications: None,
        lot_watch_reminders: None,
        metrics: auction_observability::noop_worker_recorder(),
        notifications: auction_application::notification_metrics::noop_notification_recorder(),
        control: WorkerControlHandle::new("test", 0),
        clock,
        worker_id: "worker-test".to_owned(),
        write_lifecycle: auction_application::operations::ExternalWriteLifecycle::default(),
        bulk_quota: None,
        switches: RuntimeSwitches::default(),
    }
}

/// 只记录断言需要的三类指标：每轮结果、未持久化错误与子任务 panic 归属；其余观测忽略。
#[derive(Default)]
pub(crate) struct RecordingMetrics {
    iterations: Mutex<Vec<(WorkerLoop, WorkerResult)>>,
    unpersisted: Mutex<Vec<(WorkerLoop, SafeErrorKind)>>,
    panics: Mutex<Vec<(WorkerLoop, WorkerPanicOwner)>>,
}

impl RecordingMetrics {
    pub(crate) fn iterations(&self) -> Vec<(WorkerLoop, WorkerResult)> {
        self.iterations.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub(crate) fn unpersisted(&self) -> Vec<(WorkerLoop, SafeErrorKind)> {
        self.unpersisted.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub(crate) fn panics(&self) -> Vec<(WorkerLoop, WorkerPanicOwner)> {
        self.panics.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl WorkerRecorder for RecordingMetrics {
    fn observe_task_panic(&self, loop_kind: WorkerLoop, owner: WorkerPanicOwner) {
        self.panics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((loop_kind, owner));
    }

    fn observe_unpersisted_error(&self, loop_kind: WorkerLoop, kind: SafeErrorKind) {
        self.unpersisted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((loop_kind, kind));
    }

    fn bind_retry_mode(&self, _loop_kind: WorkerLoop, _retry_kind: WorkerRetryKind) {}

    fn observe_iteration(
        &self,
        loop_kind: WorkerLoop,
        result: WorkerResult,
        _duration: std::time::Duration,
    ) {
        self.iterations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((loop_kind, result));
    }

    fn observe_batch(&self, _loop_kind: WorkerLoop, _items: usize, _capacity: usize) {}

    fn observe_retry(&self, _loop_kind: WorkerLoop, _retry_kind: WorkerRetryKind) {}

    fn observe_schedule_delay(&self, _loop_kind: WorkerLoop, _delay: std::time::Duration) {}
}

/// 排空活动交给调用方时间线的事件，与协调器观察者的登记、释放回调一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainEvent {
    /// 首次登记交接或关键段；此后关闭入口不再取消该活动。
    Entered,
    /// 已登记的活动释放，排空不再等待它；`persisted` 为 false 表示未经确认提交即被丢弃。
    Released { persisted: bool },
}

/// 排空协议替身，按装配根排空协调器的活动相位建模：准备期活动在关闭入口后取消；已登记
/// 交接或关键段的活动不受关闭影响，继续由原 owner 驱动；`complete` 或丢弃使活动进入已释放
/// 相位，此后不能再登记，关闭期间同样被取消，所以提前释放会取消仍挂起的结果落库。
/// 不复制协调器的入口锁与零活动等待：测试在单线程运行时内驱动，关闭与登记不会并发交错；
/// 排空能否结束由调用方按时间线上的释放事件断言。
#[derive(Clone, Default)]
pub(crate) struct TestDrain(Arc<DrainState>);

#[derive(Default)]
struct DrainState {
    closing: AtomicBool,
    /// 准备期活动注册的关闭唤醒；关闭时全部唤醒并清空。
    wakers: Mutex<Vec<Waker>>,
    /// 调用方时间线；未设置时事件只改变相位，不做记录。
    observer: Option<Box<dyn Fn(DrainEvent) + Send + Sync>>,
}

impl DrainState {
    fn observe(&self, event: DrainEvent) {
        if let Some(observer) = &self.observer {
            observer(event);
        }
    }
}

impl TestDrain {
    /// 把登记与释放事件交给调用方，与其它替身的调用落在同一条有序时间线上。
    pub(crate) fn observed(observer: impl Fn(DrainEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(DrainState { observer: Some(Box::new(observer)), ..DrainState::default() }))
    }

    /// 注入 Worker 的生命周期入口；每个工作项都从同一替身取得活动。
    pub(crate) fn lifecycle(&self) -> ExternalWriteLifecycle {
        ExternalWriteLifecycle::new(Arc::new(self.clone()))
    }

    /// 关闭入口并唤醒准备期活动；与装配根一致，应先于 Worker 标记排空调用。
    pub(crate) fn close(&self) {
        self.0.closing.store(true, Ordering::SeqCst);
        let wakers = std::mem::take(
            &mut *self.0.wakers.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for waker in wakers {
            waker.wake();
        }
    }
}

impl ExternalWriteActivityFactory for TestDrain {
    fn activity(&self, _kind: CriticalWriteKind) -> ExternalWriteActivity {
        ExternalWriteActivity::new(Arc::new(TestActivity {
            state: Arc::clone(&self.0),
            phase: Mutex::new(Phase::Preparing),
        }))
    }
}

/// 单个活动的相位，与协调器的准备期、已登记（交接或关键段）、已释放一一对应。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Critical,
    Finished,
}

struct TestActivity {
    state: Arc<DrainState>,
    phase: Mutex<Phase>,
}

impl TestActivity {
    fn phase(&self) -> Phase {
        *self.phase.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 进入已释放相位；与协调器的计数口径一致，只有已登记的活动释放时才产生事件。
    fn release(&self, persisted: bool) {
        let previous = std::mem::replace(
            &mut *self.phase.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            Phase::Finished,
        );
        if previous == Phase::Critical {
            self.state.observe(DrainEvent::Released { persisted });
        }
    }
}

impl Drop for TestActivity {
    fn drop(&mut self) {
        // 与协调器一致：未经 complete 就丢弃的已登记活动只记为未确认结果的释放。
        self.release(false);
    }
}

impl ExternalWriteActivityPort for TestActivity {
    fn enter_critical(&self) -> bool {
        let mut phase = self.phase.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match *phase {
            Phase::Critical => return true,
            // 已释放的活动不能重新登记；关闭入口后准备期活动也不能首次登记。
            Phase::Finished => return false,
            Phase::Preparing if self.state.closing.load(Ordering::SeqCst) => return false,
            Phase::Preparing => *phase = Phase::Critical,
        }
        drop(phase);
        self.state.observe(DrainEvent::Entered);
        true
    }

    fn complete(&self) {
        self.release(true);
    }

    fn poll_cancelled(&self, context: &mut Context<'_>) -> Poll<()> {
        if self.phase() == Phase::Critical {
            return Poll::Pending;
        }
        if self.is_cancelled() {
            return Poll::Ready(());
        }
        let mut wakers =
            self.state.wakers.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !wakers.iter().any(|waker| waker.will_wake(context.waker())) {
            wakers.push(context.waker().clone());
        }
        Poll::Pending
    }

    fn is_cancelled(&self) -> bool {
        self.state.closing.load(Ordering::SeqCst) && self.phase() != Phase::Critical
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use auction_application::operations::CriticalWriteKind;

    use super::{DrainEvent, TestDrain};

    #[tokio::test]
    async fn activity_phases_follow_drain_coordinator() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let drain = TestDrain::observed(move |event| sink.lock().unwrap().push(event));
        let lifecycle = drain.lifecycle();
        let preparing = lifecycle.activity(CriticalWriteKind::DeliverySend);
        let registered = lifecycle.activity(CriticalWriteKind::DeliverySend);
        let abandoned = lifecycle.activity(CriticalWriteKind::DeliverySend);
        assert!(registered.enter_critical() && abandoned.enter_critical());
        drain.close();

        // 关闭只取消准备期活动，它也不能再首次登记；已登记的活动继续驱动。
        assert!(preparing.is_cancelled() && !preparing.enter_critical());
        assert_eq!(preparing.run(std::future::pending::<()>()).await, None);
        assert!(!registered.is_cancelled());
        assert_eq!(registered.run(async { "persisted" }).await, Some("persisted"));

        // 释放后与准备期同样被取消，也不能重新登记：提前释放会取消仍挂起的结果落库。
        registered.complete();
        registered.complete();
        assert!(registered.is_cancelled() && !registered.enter_critical());
        assert_eq!(registered.run(std::future::pending::<()>()).await, None);

        // 只有已登记的活动释放时产生事件；未经确认即丢弃记为未确认结果的释放。
        drop((preparing, registered, abandoned));
        assert_eq!(
            *events.lock().unwrap(),
            [
                DrainEvent::Entered,
                DrainEvent::Entered,
                DrainEvent::Released { persisted: true },
                DrainEvent::Released { persisted: false },
            ]
        );
    }
}
