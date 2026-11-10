//! 指标协议类型、固定 label 和 no-op 端口。

use std::sync::Arc;

use auction_application::core::diagnostics::SafeErrorKind;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MetricsError {
    /// 注册表或指标定义失败；通常表示程序自身的指标命名冲突。
    #[error("初始化 Prometheus 指标失败")]
    Registry(#[source] prometheus::Error),
    /// Prometheus 文本编码失败。
    #[error("编码 Prometheus 指标失败")]
    Encode(#[source] prometheus::Error),
    #[error("Prometheus 指标未启用")]
    Disabled,
}

pub const METRICS_ROUTE: &str = "/metrics";

/// 数据库连接池的固定后端集合。
/// 后端名只用于选择稳定的指标族，不接受配置文件、连接串或数据库名称作为 label。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolMetricsBackend {
    Postgres,
}

impl PoolMetricsBackend {
    pub(super) const fn metric_prefix(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
        }
    }
}

/// 一次 scrape 时读取的连接池快照。
/// SQLx 原生池没有公开旧版 pgx 的累计统计，因此 `auction-storage` 在池的
/// `Executor`、事务和显式 acquisition 边界维护这些字段；没有接入追踪的外部池只能返回默认值。
#[derive(Debug, Clone, Copy, Default)]
pub struct PoolMetricsSnapshot {
    pub acquired_connections: u32,
    pub idle_connections: u32,
    pub total_connections: u32,
    pub max_connections: u32,
    pub constructing_connections: u32,
    pub acquires: u64,
    pub acquire_duration_nanos: u64,
    pub empty_acquires: u64,
    pub empty_acquire_wait_duration_nanos: u64,
    pub canceled_acquires: u64,
}

/// 数据库适配器向 Prometheus collector 提供池状态的窄端口。
pub trait PoolMetricsSource: Send + Sync {
    fn backend(&self) -> PoolMetricsBackend;
    fn snapshot(&self) -> PoolMetricsSnapshot;
}

/// 业务指标允许的固定事件集合。
/// 事件名称只表达稳定的业务边界，不接受请求、用户、拍品或外部服务返回的动态值作为
/// label。需要更细的业务审计时，应读取数据库事实和审计记录，而不是扩张 Prometheus
/// label 的基数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusinessEvent {
    Bid,
    Proxy,
    MailIntake,
    MailDelivery,
    AuctionLifecycle,
    /// 结算阶段已从拍卖主循环拆为独立任务，单独计数轮次与结果。
    Settlement,
    Fulfillment,
    /// recovery 退回、重列或退款动作。
    Recovery,
    ForumSync,
    OperationsAlert,
}

impl BusinessEvent {
    pub(super) const ALL: [Self; 10] = [
        Self::Bid,
        Self::Proxy,
        Self::MailIntake,
        Self::MailDelivery,
        Self::AuctionLifecycle,
        Self::Settlement,
        Self::Fulfillment,
        Self::Recovery,
        Self::ForumSync,
        Self::OperationsAlert,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Bid => "bid",
            Self::Proxy => "proxy",
            Self::MailIntake => "mail-intake",
            Self::MailDelivery => "mail-delivery",
            Self::AuctionLifecycle => "auction-lifecycle",
            Self::Settlement => "settlement",
            Self::Fulfillment => "fulfillment",
            Self::Recovery => "recovery",
            Self::ForumSync => "forum-sync",
            Self::OperationsAlert => "operations-alert",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusinessOutcome {
    Success,
    /// 请求被领域规则、状态或幂等冲突拒绝。
    Rejected,
    /// 适配器、存储或外部服务失败。
    Error,
}

impl BusinessOutcome {
    pub(super) const ALL: [Self; 3] = [Self::Success, Self::Rejected, Self::Error];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Rejected => "rejected",
            Self::Error => "error",
        }
    }
}

/// 业务指标的低基数记录端口。
/// HTTP、Worker 和其他表面只依赖这个窄端口；Prometheus 注册表和 exposition 格式仍由
/// 可观测性 crate 持有。关闭指标时由 no-op 实现吸收调用。
pub trait BusinessRecorder: Send + Sync {
    fn observe(&self, event: BusinessEvent, outcome: BusinessOutcome);
    fn observe_duration(&self, event: BusinessEvent, duration: std::time::Duration);
}

#[derive(Debug, Default)]
struct NoopBusinessRecorder;

impl BusinessRecorder for NoopBusinessRecorder {
    fn observe(&self, _event: BusinessEvent, _outcome: BusinessOutcome) {}

    fn observe_duration(&self, _event: BusinessEvent, _duration: std::time::Duration) {}
}

pub(super) fn noop_business_recorder() -> Arc<dyn BusinessRecorder> {
    Arc::new(NoopBusinessRecorder)
}

/// Worker 指标允许的固定循环集合。
/// 枚举会被映射到固定 label，调用方不能把用户输入、数据库主键或外部错误文本带入指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerLoop {
    MailMain,
    MailIsekai,
    /// 主世界独立维护阶段，不映射为业务收件事件。
    MailMaintenanceMain,
    /// 异世界独立维护阶段，不映射为业务收件事件。
    MailMaintenanceIsekai,
    /// 主世界完整 Read Mail 纯观察，不映射为业务收件事件。
    MailReadObservationMain,
    /// 异世界完整 Read Mail 纯观察，不映射为业务收件事件。
    MailReadObservationIsekai,
    Delivery,
    Auction,
    /// 结算阶段独立循环：结算耗时不再推迟另一世界的生命周期与慢速代理推进。
    Settlement,
    /// 结算履约准备与对账。
    AuctionFulfillment,
    AuctionRecovery,
    Forum,
    OperationsAlertScan,
    OperationsAlertDispatch,
    OperationsAlertReconcile,
    /// 卖方汇总与换季提醒的玩家通知扫描。
    PlayerNotifications,
}

impl WorkerLoop {
    /// 初始化固定循环的零值序列，不接收运行时 label。
    pub(super) const ALL: [Self; 16] = [
        Self::MailMain,
        Self::MailIsekai,
        Self::MailMaintenanceMain,
        Self::MailMaintenanceIsekai,
        Self::MailReadObservationMain,
        Self::MailReadObservationIsekai,
        Self::Delivery,
        Self::Auction,
        Self::Settlement,
        Self::AuctionFulfillment,
        Self::AuctionRecovery,
        Self::Forum,
        Self::OperationsAlertScan,
        Self::OperationsAlertDispatch,
        Self::OperationsAlertReconcile,
        Self::PlayerNotifications,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::MailMain => "mail-main",
            Self::MailIsekai => "mail-isekai",
            Self::MailMaintenanceMain => "mail-maintenance-main",
            Self::MailMaintenanceIsekai => "mail-maintenance-isekai",
            Self::MailReadObservationMain => "mail-read-observation-main",
            Self::MailReadObservationIsekai => "mail-read-observation-isekai",
            Self::Delivery => "delivery",
            Self::Auction => "auction",
            Self::Settlement => "settlement",
            Self::AuctionFulfillment => "auction-fulfillment",
            Self::AuctionRecovery => "auction-recovery",
            Self::Forum => "forum",
            Self::OperationsAlertScan => "operations-alert-scan",
            Self::OperationsAlertDispatch => "operations-alert-dispatch",
            Self::OperationsAlertReconcile => "operations-alert-reconcile",
            Self::PlayerNotifications => "player-notifications",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerResult {
    Success,
    /// 本轮遇到错误并已按所属 owner 记录。
    Error,
    /// 本轮被取消或任务 join 未完成。
    Canceled,
}

impl WorkerResult {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerRetryKind {
    Inline,
    /// 通过数据库 lease/next_attempt_at 持久化后重试。
    Persisted,
}

impl WorkerRetryKind {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Persisted => "persisted",
        }
    }
}

/// 工作项 panic 的固定 owner；不接受工作项 ID 或动态指标 label。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerPanicOwner {
    Lifecycle,
    Settlement,
    SlowProxy,
    Fulfillment,
    RecoveryAction,
    Delivery,
    CriticalLoop,
}

impl WorkerPanicOwner {
    pub const ALL: [Self; 7] = [
        Self::Lifecycle,
        Self::Settlement,
        Self::SlowProxy,
        Self::Fulfillment,
        Self::RecoveryAction,
        Self::Delivery,
        Self::CriticalLoop,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lifecycle => "lifecycle",
            Self::Settlement => "settlement",
            Self::SlowProxy => "slow_proxy",
            Self::Fulfillment => "fulfillment",
            Self::RecoveryAction => "recovery_action",
            Self::Delivery => "delivery",
            Self::CriticalLoop => "critical_loop",
        }
    }
}

/// Worker 循环使用的低基数指标端口。
/// Worker crate 只依赖这组窄操作，不知道 Prometheus 注册表和 exposition 格式；关闭指标
/// 时使用同一接口的 no-op 实现，避免在业务循环中散落条件分支。
pub trait WorkerRecorder: Send + Sync {
    /// 现有工作项子任务在 join 收口时观察到的 panic；不写入持久尝试次数。
    fn observe_task_panic(&self, loop_kind: WorkerLoop, owner: WorkerPanicOwner);

    /// 没有确认对应持久失败、退避或人工处置的错误；不接收动态 label。
    fn observe_unpersisted_error(&self, loop_kind: WorkerLoop, kind: SafeErrorKind);

    fn bind_retry_mode(&self, loop_kind: WorkerLoop, retry_kind: WorkerRetryKind);
    fn observe_iteration(
        &self,
        loop_kind: WorkerLoop,
        result: WorkerResult,
        duration: std::time::Duration,
    );
    fn observe_batch(&self, loop_kind: WorkerLoop, items: usize, capacity: usize);
    fn observe_retry(&self, loop_kind: WorkerLoop, retry_kind: WorkerRetryKind);
    fn observe_schedule_delay(&self, loop_kind: WorkerLoop, delay: std::time::Duration);
}

#[derive(Debug, Default)]
struct NoopWorkerRecorder;

impl WorkerRecorder for NoopWorkerRecorder {
    fn observe_task_panic(&self, _loop_kind: WorkerLoop, _owner: WorkerPanicOwner) {}

    fn observe_unpersisted_error(&self, _loop_kind: WorkerLoop, _kind: SafeErrorKind) {}

    fn bind_retry_mode(&self, _loop_kind: WorkerLoop, _retry_kind: WorkerRetryKind) {}

    fn observe_iteration(
        &self,
        _loop_kind: WorkerLoop,
        _result: WorkerResult,
        _duration: std::time::Duration,
    ) {
    }

    fn observe_batch(&self, _loop_kind: WorkerLoop, _items: usize, _capacity: usize) {}

    fn observe_retry(&self, _loop_kind: WorkerLoop, _retry_kind: WorkerRetryKind) {}

    fn observe_schedule_delay(&self, _loop_kind: WorkerLoop, _delay: std::time::Duration) {}
}

pub fn noop_worker_recorder() -> Arc<dyn WorkerRecorder> {
    Arc::new(NoopWorkerRecorder)
}
