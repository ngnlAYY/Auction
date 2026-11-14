//! 循环级回归测试的最小 Worker 装配：跟随 Tokio 暂停时间的时钟、失败关闭的惰性端口、
//! 已校验的热参数与可断言的指标记录。只在 `cfg(test)` 编译，不进入运行时制品。

use std::sync::{Arc, Mutex};

use auction_application::admin::{
    WorkerDeliveryHotConfig, WorkerForumHotConfig, WorkerHotConfig, WorkerMailHotConfig,
};
use auction_application::auction::{
    ApplicationFuture, AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecycleResult,
};
use auction_application::core::diagnostics::SafeErrorKind;
use auction_application::core::{ApplicationError, Clock};
use auction_application::delivery::DeliverySendIntervals;
use auction_application::mail::MailSendClasses;
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

/// 只记录断言需要的两类指标：每轮结果与未持久化错误；其余观测忽略。
#[derive(Default)]
pub(crate) struct RecordingMetrics {
    iterations: Mutex<Vec<(WorkerLoop, WorkerResult)>>,
    unpersisted: Mutex<Vec<(WorkerLoop, SafeErrorKind)>>,
}

impl RecordingMetrics {
    pub(crate) fn iterations(&self) -> Vec<(WorkerLoop, WorkerResult)> {
        self.iterations.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub(crate) fn unpersisted(&self) -> Vec<(WorkerLoop, SafeErrorKind)> {
        self.unpersisted.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

impl WorkerRecorder for RecordingMetrics {
    fn observe_task_panic(&self, _loop_kind: WorkerLoop, _owner: WorkerPanicOwner) {}

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
