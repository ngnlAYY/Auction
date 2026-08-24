//! 指标协议类型、固定 label 和 no-op 端口。

use std::sync::Arc;

use thiserror::Error;

/// 进程本地指标输出错误。
#[derive(Debug, Error)]
pub enum MetricsError {
    /// 注册表或指标定义失败；通常表示程序自身的指标命名冲突。
    #[error("初始化 Prometheus 指标失败")]
    Registry(#[source] prometheus::Error),
    /// Prometheus 文本编码失败。
    #[error("编码 Prometheus 指标失败")]
    Encode(#[source] prometheus::Error),
    /// 当前进程未启用指标导出。
    #[error("Prometheus 指标未启用")]
    Disabled,
}

/// 固定的进程本地 Prometheus scrape 路径。
pub const METRICS_ROUTE: &str = "/metrics";

/// 数据库连接池的固定后端集合。
///
/// 后端名只用于选择稳定的指标族，不接受配置文件、连接串或数据库名称作为 label。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolMetricsBackend {
    /// PostgreSQL 完整档连接池。
    Postgres,
    /// SQLite Lite 档连接池。
    Sqlite,
}

impl PoolMetricsBackend {
    pub(super) const fn metric_prefix(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
        }
    }
}

/// 一次 scrape 时读取的连接池快照。
///
/// SQLx 原生池没有公开旧版 pgx 的累计统计，因此 `auction-storage` 在池的
/// `Executor`、事务和显式 acquisition 边界维护这些字段；没有接入追踪的外部池只能返回默认值。
#[derive(Debug, Clone, Copy, Default)]
pub struct PoolMetricsSnapshot {
    /// 当前已借出的连接数。
    pub acquired_connections: u32,
    /// 当前空闲连接数。
    pub idle_connections: u32,
    /// 当前池内连接总数。
    pub total_connections: u32,
    /// 连接池允许的最大连接数。
    pub max_connections: u32,
    /// 当前正在尝试扩容连接的 acquisition 数量。
    pub constructing_connections: u32,
    /// 成功获取连接的累计次数。
    pub acquires: u64,
    /// 成功获取连接所耗费的累计纳秒数。
    pub acquire_duration_nanos: u64,
    /// 获取开始时没有空闲连接的累计次数。
    pub empty_acquires: u64,
    /// 空池获取等待的累计纳秒数。
    pub empty_acquire_wait_duration_nanos: u64,
    /// acquisition future 在完成前被取消的累计次数。
    pub canceled_acquires: u64,
}

/// 数据库适配器向 Prometheus collector 提供池状态的窄端口。
pub trait PoolMetricsSource: Send + Sync {
    /// 返回固定的数据库后端类型。
    fn backend(&self) -> PoolMetricsBackend;
    /// 返回当前 scrape 时刻的连接池状态。
    fn snapshot(&self) -> PoolMetricsSnapshot;
}

/// 业务指标允许的固定事件集合。
///
/// 事件名称只表达稳定的业务边界，不接受请求、用户、拍品或外部服务返回的动态值作为
/// label。需要更细的业务审计时，应读取数据库事实和审计记录，而不是扩张 Prometheus
/// label 的基数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusinessEvent {
    /// 普通竞价命令。
    Bid,
    /// 代理授权创建或降额命令。
    Proxy,
    /// MoogleMail 入站解析与导入。
    MailIntake,
    /// MoogleMail 外发投递。
    MailDelivery,
    /// 拍卖生命周期调度。
    AuctionLifecycle,
    /// 结算履约动作。
    Fulfillment,
    /// recovery 退回、重列或退款动作。
    Recovery,
    /// 论坛同步或回帖通知。
    ForumSync,
    /// 实时广播与重连补发。
    Realtime,
    /// 运维告警扫描、分发或对账。
    OperationsAlert,
}

impl BusinessEvent {
    pub(super) const ALL: [Self; 10] = [
        Self::Bid,
        Self::Proxy,
        Self::MailIntake,
        Self::MailDelivery,
        Self::AuctionLifecycle,
        Self::Fulfillment,
        Self::Recovery,
        Self::ForumSync,
        Self::Realtime,
        Self::OperationsAlert,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Bid => "bid",
            Self::Proxy => "proxy",
            Self::MailIntake => "mail-intake",
            Self::MailDelivery => "mail-delivery",
            Self::AuctionLifecycle => "auction-lifecycle",
            Self::Fulfillment => "fulfillment",
            Self::Recovery => "recovery",
            Self::ForumSync => "forum-sync",
            Self::Realtime => "realtime",
            Self::OperationsAlert => "operations-alert",
        }
    }
}

/// 业务事件允许的固定结果集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusinessOutcome {
    /// 业务命令或 Worker 动作成功完成。
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
///
/// HTTP、Worker 和其他表面只依赖这个窄端口；Prometheus 注册表和 exposition 格式仍由
/// 可观测性 crate 持有。关闭指标时由 no-op 实现吸收调用。
pub trait BusinessRecorder: Send + Sync {
    /// 记录一个固定业务事件的结果。
    fn observe(&self, event: BusinessEvent, outcome: BusinessOutcome);
    /// 记录一个固定业务事件的处理耗时。
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
///
/// 枚举会被映射到固定 label，调用方不能把用户输入、数据库主键或外部错误文本带入指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerLoop {
    /// main 世界 MoogleMail 入站轮询。
    MailMain,
    /// isekai 世界 MoogleMail 入站轮询。
    MailIsekai,
    /// MoogleMail 外发 delivery。
    Delivery,
    /// 拍卖生命周期调度。
    Auction,
    /// 结算履约准备与对账。
    AuctionFulfillment,
    /// recovery 退回、重列和退款动作。
    AuctionRecovery,
    /// operations alert 观察扫描。
    OperationsAlertScan,
    /// operations alert 通知批次分发。
    OperationsAlertDispatch,
    /// operations alert 已发送批次对账。
    OperationsAlertReconcile,
}

impl WorkerLoop {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::MailMain => "mail-main",
            Self::MailIsekai => "mail-isekai",
            Self::Delivery => "delivery",
            Self::Auction => "auction",
            Self::AuctionFulfillment => "auction-fulfillment",
            Self::AuctionRecovery => "auction-recovery",
            Self::OperationsAlertScan => "operations-alert-scan",
            Self::OperationsAlertDispatch => "operations-alert-dispatch",
            Self::OperationsAlertReconcile => "operations-alert-reconcile",
        }
    }
}

/// Worker 单次循环的固定结果集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerResult {
    /// 本轮成功完成。
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

/// Worker 持久化或内联重试路径的固定集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerRetryKind {
    /// 当前循环内立即重试。
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

/// Worker 循环使用的低基数指标端口。
///
/// Worker crate 只依赖这组窄操作，不知道 Prometheus 注册表和 exposition 格式；关闭指标
/// 时使用同一接口的 no-op 实现，避免在业务循环中散落条件分支。
pub trait WorkerRecorder: Send + Sync {
    /// 声明某个循环实际绑定的重试模式。
    fn bind_retry_mode(&self, loop_kind: WorkerLoop, retry_kind: WorkerRetryKind);
    /// 记录一次循环结果和持续时间。
    fn observe_iteration(
        &self,
        loop_kind: WorkerLoop,
        result: WorkerResult,
        duration: std::time::Duration,
    );
    /// 记录一次成功领取的批次、实际项目数和容量。
    fn observe_batch(&self, loop_kind: WorkerLoop, items: usize, capacity: usize);
    /// 记录一次持久化或内联重试。
    fn observe_retry(&self, loop_kind: WorkerLoop, retry_kind: WorkerRetryKind);
    /// 记录调度时间到实际开始时间的延迟。
    fn observe_schedule_delay(&self, loop_kind: WorkerLoop, delay: std::time::Duration);
    /// 记录一个由 Worker 直接完成的业务动作。
    fn observe_business(&self, _event: BusinessEvent, _outcome: BusinessOutcome) {}
    /// 记录一个由 Worker 直接完成的业务动作耗时。
    fn observe_business_duration(&self, _event: BusinessEvent, _duration: std::time::Duration) {}
}

#[derive(Debug, Default)]
struct NoopWorkerRecorder;

impl WorkerRecorder for NoopWorkerRecorder {
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

/// 返回关闭状态的 Worker recorder。
pub fn noop_worker_recorder() -> Arc<dyn WorkerRecorder> {
    Arc::new(NoopWorkerRecorder)
}
