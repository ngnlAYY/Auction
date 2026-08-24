//! 进程本地 Prometheus 指标注册表与固定 scrape 输出。
//!
//! 指标注册表按进程独立创建，不写入 Prometheus 全局注册表，也不接受运行期任意 label。
//! 这样 `/metrics` 只能暴露装配根提供的服务身份和少量进程状态，不会把 Cookie、路径或
//! 数据库内容带入指标标签。

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::health::{TracingHealth, TracingHealthReader, TracingStatus};
use prometheus::{
    Counter, CounterVec, Encoder, Gauge, GaugeVec, HistogramOpts, HistogramVec, IntCounter, Opts,
    Registry, TextEncoder,
    core::{Collector, Desc},
    process_collector::ProcessCollector,
    proto::MetricFamily,
};
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
    const fn metric_prefix(self) -> &'static str {
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
    const ALL: [Self; 10] = [
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

    const fn label(self) -> &'static str {
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
    const ALL: [Self; 3] = [Self::Success, Self::Rejected, Self::Error];

    const fn label(self) -> &'static str {
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

fn noop_business_recorder() -> Arc<dyn BusinessRecorder> {
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
    const fn label(self) -> &'static str {
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
    const fn label(self) -> &'static str {
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
    const fn label(self) -> &'static str {
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

/// 一个进程拥有的自定义 Prometheus 注册表。
///
/// Public/Admin 暴露进程资源、进程存活、指标开关和安全构建身份；Worker 额外注册固定 label
/// 的循环指标。这里不读取数据库，也不拼接请求、用户或外部错误等高基数标签。
pub struct ProcessMetrics {
    enabled: bool,
    registry: Registry,
    worker: Arc<dyn WorkerRecorder>,
    business: Arc<dyn BusinessRecorder>,
}

impl ProcessMetrics {
    /// 创建进程指标注册表。关闭时不注册 collector，也不会建立后台任务。
    pub fn new(
        enabled: bool,
        service: &'static str,
        role: &'static str,
        version: &'static str,
    ) -> Result<Self, MetricsError> {
        Self::new_with_tracing_health(
            enabled,
            service,
            role,
            version,
            Arc::new(DisabledTracingHealth),
        )
    }

    /// 创建带 tracing 安全状态读取器的进程指标注册表。
    ///
    /// tracing 指标只读取固定状态字段，不把 exporter endpoint、headers 或 span 内容暴露给
    /// Prometheus；Public/Worker 没有 exporter 时传入的关闭读取器仍会产生稳定的零值族。
    pub fn new_with_tracing_health(
        enabled: bool,
        service: &'static str,
        role: &'static str,
        version: &'static str,
        tracing_health: Arc<dyn TracingHealthReader>,
    ) -> Result<Self, MetricsError> {
        if !enabled {
            return Ok(Self::disabled());
        }
        if !valid_identity(service) || !valid_identity(role) || !valid_identity(version) {
            return Err(MetricsError::Registry(prometheus::Error::Msg(
                "metrics identity is invalid".to_owned(),
            )));
        }

        let registry = Registry::new();
        let process_up = Gauge::with_opts(Opts::new(
            "auction_process_up",
            "Whether the process has completed telemetry initialization.",
        ))
        .map_err(MetricsError::Registry)?;
        process_up.set(1.0);
        registry.register(Box::new(process_up)).map_err(MetricsError::Registry)?;

        let metrics_enabled = Gauge::with_opts(Opts::new(
            "auction_metrics_enabled",
            "Whether the process-local Prometheus exporter is enabled.",
        ))
        .map_err(MetricsError::Registry)?;
        metrics_enabled.set(1.0);
        registry.register(Box::new(metrics_enabled)).map_err(MetricsError::Registry)?;

        let process_info = GaugeVec::new(
            Opts::new("auction_process_info", "Static process identity information."),
            &["service", "role", "version"],
        )
        .map_err(MetricsError::Registry)?;
        process_info.with_label_values(&[service, role, version]).set(1.0);
        registry.register(Box::new(process_info)).map_err(MetricsError::Registry)?;

        // 旧版 registry 同时提供 CPU、内存、文件描述符、线程数和启动时间等进程资源指标。
        // ProcessCollector 只读取当前进程，不带连接串、请求参数或业务主键，适合直接复用。
        registry
            .register(Box::new(ProcessCollector::for_self()))
            .map_err(MetricsError::Registry)?;

        let tracing = TracingCollector::new(tracing_health)?;
        registry.register(Box::new(tracing)).map_err(MetricsError::Registry)?;

        let business = PrometheusBusinessRecorder::build(&registry)?;
        let worker = if role == "worker" {
            PrometheusWorkerRecorder::build(&registry, Arc::clone(&business))?
        } else {
            noop_worker_recorder()
        };

        Ok(Self { enabled: true, registry, worker, business })
    }

    /// 创建关闭状态的指标对象；关闭时不分配注册表或后台资源。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            registry: Registry::new(),
            worker: noop_worker_recorder(),
            business: noop_business_recorder(),
        }
    }

    /// 返回指标是否启用。
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// 以 Prometheus text exposition format 生成一次完整快照。
    pub fn encode(&self) -> Result<Vec<u8>, MetricsError> {
        if !self.enabled {
            return Err(MetricsError::Disabled);
        }
        let families = self.registry.gather();
        let mut output = Vec::new();
        TextEncoder::new().encode(&families, &mut output).map_err(MetricsError::Encode)?;
        Ok(output)
    }

    /// 返回 scrape 响应的稳定 Content-Type。
    pub const fn content_type() -> &'static str {
        "text/plain; version=0.0.4; charset=utf-8"
    }

    /// 返回当前进程的 Worker 指标端口；Public/Admin 会得到 no-op 实现。
    pub fn worker(&self) -> Arc<dyn WorkerRecorder> {
        Arc::clone(&self.worker)
    }

    /// 返回当前进程的业务指标端口。
    pub fn business(&self) -> Arc<dyn BusinessRecorder> {
        Arc::clone(&self.business)
    }

    /// 注册数据库连接池状态 collector。
    ///
    /// 数据库通常在 HTTP/Worker 装配中晚于进程指标对象建立，因此允许在连接成功后注册一次。
    /// collector 只读取池的内存快照，不执行 SQL，也不把连接串或数据库身份带入指标。
    pub fn register_pool_metrics(
        &self,
        source: Arc<dyn PoolMetricsSource>,
    ) -> Result<(), MetricsError> {
        if !self.enabled {
            return Ok(());
        }
        self.registry
            .register(Box::new(PoolMetricsCollector::new(source)?))
            .map_err(MetricsError::Registry)
    }
}

/// Prometheus 业务指标的固定 label 适配器。
struct PrometheusBusinessRecorder {
    events: CounterVec,
    durations: HistogramVec,
}

impl PrometheusBusinessRecorder {
    fn build(registry: &Registry) -> Result<Arc<dyn BusinessRecorder>, MetricsError> {
        let events = CounterVec::new(
            Opts::new(
                "auction_business_events_total",
                "Total business events by fixed event and outcome.",
            ),
            &["event", "outcome"],
        )
        .map_err(MetricsError::Registry)?;
        let durations = HistogramVec::new(
            HistogramOpts::new(
                "auction_business_event_duration_seconds",
                "Business event processing duration in seconds.",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 30.0]),
            &["event"],
        )
        .map_err(MetricsError::Registry)?;

        // 预先建立闭集的零值，确保 scrape 在业务尚未发生时也能看到完整指标族。
        for event in BusinessEvent::ALL {
            durations.with_label_values(&[event.label()]);
            for outcome in BusinessOutcome::ALL {
                events.with_label_values(&[event.label(), outcome.label()]);
            }
        }
        registry.register(Box::new(events.clone())).map_err(MetricsError::Registry)?;
        registry.register(Box::new(durations.clone())).map_err(MetricsError::Registry)?;

        Ok(Arc::new(Self { events, durations }))
    }
}

impl BusinessRecorder for PrometheusBusinessRecorder {
    fn observe(&self, event: BusinessEvent, outcome: BusinessOutcome) {
        self.events.with_label_values(&[event.label(), outcome.label()]).inc();
    }

    fn observe_duration(&self, event: BusinessEvent, duration: std::time::Duration) {
        self.durations.with_label_values(&[event.label()]).observe(duration.as_secs_f64());
    }
}

/// SQLx 当前公开能力对应的连接池 Prometheus collector。
struct PoolMetricsCollector {
    source: Arc<dyn PoolMetricsSource>,
    acquired_connections: Gauge,
    idle_connections: Gauge,
    total_connections: Gauge,
    max_connections: Gauge,
    constructing_connections: Gauge,
    acquires: IntCounter,
    acquire_duration: Counter,
    empty_acquires: IntCounter,
    empty_acquire_wait_duration: Counter,
    canceled_acquires: IntCounter,
    last_acquires: AtomicU64,
    last_acquire_duration_nanos: AtomicU64,
    last_empty_acquires: AtomicU64,
    last_empty_acquire_wait_duration_nanos: AtomicU64,
    last_canceled_acquires: AtomicU64,
}

impl PoolMetricsCollector {
    fn new(source: Arc<dyn PoolMetricsSource>) -> Result<Self, MetricsError> {
        let prefix = source.backend().metric_prefix();
        let metric = |name: &str, help: &str| {
            Gauge::with_opts(Opts::new(format!("auction_{prefix}_pool_{name}"), help))
                .map_err(MetricsError::Registry)
        };

        Ok(Self {
            source,
            acquired_connections: metric(
                "acquired_connections",
                "Current number of database pool connections checked out.",
            )?,
            idle_connections: metric(
                "idle_connections",
                "Current number of idle database pool connections.",
            )?,
            total_connections: metric(
                "total_connections",
                "Current number of database pool connections.",
            )?,
            max_connections: metric(
                "max_connections",
                "Configured maximum number of database pool connections.",
            )?,
            constructing_connections: metric(
                "constructing_connections",
                "Current number of tracked database pool connection growth attempts.",
            )?,
            acquires: IntCounter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_acquires_total"),
                "Total successful database pool acquires.",
            ))
            .map_err(MetricsError::Registry)?,
            acquire_duration: Counter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_acquire_duration_seconds_total"),
                "Total time spent acquiring database pool connections in seconds.",
            ))
            .map_err(MetricsError::Registry)?,
            empty_acquires: IntCounter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_empty_acquires_total"),
                "Total database pool acquires that started without an idle connection.",
            ))
            .map_err(MetricsError::Registry)?,
            empty_acquire_wait_duration: Counter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_empty_acquire_wait_duration_seconds_total"),
                "Total wait time for database pool acquires that started without an idle connection.",
            ))
            .map_err(MetricsError::Registry)?,
            canceled_acquires: IntCounter::with_opts(Opts::new(
                format!("auction_{prefix}_pool_canceled_acquires_total"),
                "Total database pool acquires canceled before completion.",
            ))
            .map_err(MetricsError::Registry)?,
            last_acquires: AtomicU64::new(0),
            last_acquire_duration_nanos: AtomicU64::new(0),
            last_empty_acquires: AtomicU64::new(0),
            last_empty_acquire_wait_duration_nanos: AtomicU64::new(0),
            last_canceled_acquires: AtomicU64::new(0),
        })
    }
}

impl Collector for PoolMetricsCollector {
    fn desc(&self) -> Vec<&Desc> {
        [
            self.acquired_connections.desc(),
            self.idle_connections.desc(),
            self.total_connections.desc(),
            self.max_connections.desc(),
            self.constructing_connections.desc(),
            self.acquires.desc(),
            self.acquire_duration.desc(),
            self.empty_acquires.desc(),
            self.empty_acquire_wait_duration.desc(),
            self.canceled_acquires.desc(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let snapshot = self.source.snapshot();
        self.acquired_connections.set(f64::from(snapshot.acquired_connections));
        self.idle_connections.set(f64::from(snapshot.idle_connections));
        self.total_connections.set(f64::from(snapshot.total_connections));
        self.max_connections.set(f64::from(snapshot.max_connections));
        self.constructing_connections.set(f64::from(snapshot.constructing_connections));
        increment_counter(&self.acquires, &self.last_acquires, snapshot.acquires, 1.0);
        increment_counter(
            &self.acquire_duration,
            &self.last_acquire_duration_nanos,
            snapshot.acquire_duration_nanos,
            1_000_000_000.0,
        );
        increment_counter(
            &self.empty_acquires,
            &self.last_empty_acquires,
            snapshot.empty_acquires,
            1.0,
        );
        increment_counter(
            &self.empty_acquire_wait_duration,
            &self.last_empty_acquire_wait_duration_nanos,
            snapshot.empty_acquire_wait_duration_nanos,
            1_000_000_000.0,
        );
        increment_counter(
            &self.canceled_acquires,
            &self.last_canceled_acquires,
            snapshot.canceled_acquires,
            1.0,
        );

        [
            self.acquired_connections.collect(),
            self.idle_connections.collect(),
            self.total_connections.collect(),
            self.max_connections.collect(),
            self.constructing_connections.collect(),
            self.acquires.collect(),
            self.acquire_duration.collect(),
            self.empty_acquires.collect(),
            self.empty_acquire_wait_duration.collect(),
            self.canceled_acquires.collect(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

fn increment_counter<C>(counter: &C, last: &AtomicU64, current: u64, divisor: f64)
where
    C: CounterIncrement,
{
    loop {
        let previous = last.load(Ordering::Relaxed);
        if current <= previous {
            return;
        }
        let delta = current - previous;
        if last.compare_exchange(previous, current, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            // 这里需要通过具体 counter 类型增加；由下面的 trait 适配器统一处理浮点与整数。
            counter_increment(counter, delta, divisor);
            return;
        }
    }
}

trait CounterIncrement {
    fn increment(&self, delta: u64, divisor: f64);
}

impl CounterIncrement for IntCounter {
    fn increment(&self, delta: u64, _divisor: f64) {
        self.inc_by(delta);
    }
}

impl CounterIncrement for Counter {
    fn increment(&self, delta: u64, divisor: f64) {
        self.inc_by(delta as f64 / divisor);
    }
}

fn counter_increment<C>(counter: &C, delta: u64, divisor: f64)
where
    C: CounterIncrement,
{
    counter.increment(delta, divisor);
}

/// Prometheus Worker 指标的固定 label 适配器。
struct PrometheusWorkerRecorder {
    iterations: CounterVec,
    durations: HistogramVec,
    batch_items: CounterVec,
    batch_capacity: CounterVec,
    batch_saturated: CounterVec,
    retry_modes: GaugeVec,
    retries: CounterVec,
    schedule_delay: HistogramVec,
    business: Arc<dyn BusinessRecorder>,
}

impl PrometheusWorkerRecorder {
    fn build(
        registry: &Registry,
        business: Arc<dyn BusinessRecorder>,
    ) -> Result<Arc<dyn WorkerRecorder>, MetricsError> {
        let iterations = CounterVec::new(
            Opts::new(
                "auction_worker_loop_iterations_total",
                "Total Worker loop iterations by fixed loop and result.",
            ),
            &["loop", "result"],
        )
        .map_err(MetricsError::Registry)?;
        let durations = HistogramVec::new(
            HistogramOpts::new(
                "auction_worker_loop_iteration_duration_seconds",
                "Worker loop iteration duration in seconds.",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0, 30.0]),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;
        let batch_items = CounterVec::new(
            Opts::new("auction_worker_batch_items_total", "Total successful Worker batch items."),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;
        let batch_capacity = CounterVec::new(
            Opts::new(
                "auction_worker_batch_capacity_total",
                "Total configured capacity of successful Worker batches.",
            ),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;
        let batch_saturated = CounterVec::new(
            Opts::new(
                "auction_worker_batch_saturated_total",
                "Total successful Worker batches that reached capacity.",
            ),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;
        let retry_modes = GaugeVec::new(
            Opts::new(
                "auction_worker_retry_mode_bound",
                "Whether a concrete Worker loop bound the supported retry mode.",
            ),
            &["loop", "kind"],
        )
        .map_err(MetricsError::Registry)?;
        let retries = CounterVec::new(
            Opts::new(
                "auction_worker_retry_attempts_total",
                "Total Worker retry attempts by fixed loop and retry kind.",
            ),
            &["loop", "kind"],
        )
        .map_err(MetricsError::Registry)?;
        let schedule_delay = HistogramVec::new(
            HistogramOpts::new(
                "auction_worker_schedule_delay_seconds",
                "Worker scheduled-to-actual start delay in seconds.",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
            &["loop"],
        )
        .map_err(MetricsError::Registry)?;

        for collector in [
            Box::new(iterations.clone()) as Box<dyn Collector>,
            Box::new(durations.clone()),
            Box::new(batch_items.clone()),
            Box::new(batch_capacity.clone()),
            Box::new(batch_saturated.clone()),
            Box::new(retry_modes.clone()),
            Box::new(retries.clone()),
            Box::new(schedule_delay.clone()),
        ] {
            registry.register(collector).map_err(MetricsError::Registry)?;
        }

        Ok(Arc::new(Self {
            iterations,
            durations,
            batch_items,
            batch_capacity,
            batch_saturated,
            retry_modes,
            retries,
            schedule_delay,
            business,
        }))
    }
}

impl WorkerRecorder for PrometheusWorkerRecorder {
    fn bind_retry_mode(&self, loop_kind: WorkerLoop, retry_kind: WorkerRetryKind) {
        if !valid_retry_mode(loop_kind, retry_kind) {
            return;
        }
        self.retry_modes.with_label_values(&[loop_kind.label(), retry_kind.label()]).set(1.0);
    }

    fn observe_iteration(
        &self,
        loop_kind: WorkerLoop,
        result: WorkerResult,
        duration: std::time::Duration,
    ) {
        self.iterations.with_label_values(&[loop_kind.label(), result.label()]).inc();
        self.durations.with_label_values(&[loop_kind.label()]).observe(duration.as_secs_f64());
        if let Some(event) = business_event_for_worker(loop_kind) {
            let outcome = match result {
                WorkerResult::Success => BusinessOutcome::Success,
                WorkerResult::Error | WorkerResult::Canceled => BusinessOutcome::Error,
            };
            self.business.observe(event, outcome);
            self.business.observe_duration(event, duration);
        }
    }

    fn observe_batch(&self, loop_kind: WorkerLoop, items: usize, capacity: usize) {
        if capacity == 0 || items > capacity {
            return;
        }
        let label = loop_kind.label();
        self.batch_items.with_label_values(&[label]).inc_by(items as f64);
        self.batch_capacity.with_label_values(&[label]).inc_by(capacity as f64);
        if items == capacity {
            self.batch_saturated.with_label_values(&[label]).inc();
        }
    }

    fn observe_retry(&self, loop_kind: WorkerLoop, retry_kind: WorkerRetryKind) {
        if !valid_retry_mode(loop_kind, retry_kind) {
            return;
        }
        self.bind_retry_mode(loop_kind, retry_kind);
        self.retries.with_label_values(&[loop_kind.label(), retry_kind.label()]).inc();
    }

    fn observe_schedule_delay(&self, loop_kind: WorkerLoop, delay: std::time::Duration) {
        self.schedule_delay.with_label_values(&[loop_kind.label()]).observe(delay.as_secs_f64());
    }

    fn observe_business(&self, event: BusinessEvent, outcome: BusinessOutcome) {
        self.business.observe(event, outcome);
    }

    fn observe_business_duration(&self, event: BusinessEvent, duration: std::time::Duration) {
        self.business.observe_duration(event, duration);
    }
}

fn business_event_for_worker(loop_kind: WorkerLoop) -> Option<BusinessEvent> {
    match loop_kind {
        WorkerLoop::MailMain | WorkerLoop::MailIsekai => Some(BusinessEvent::MailIntake),
        WorkerLoop::Delivery => Some(BusinessEvent::MailDelivery),
        WorkerLoop::Auction => Some(BusinessEvent::AuctionLifecycle),
        WorkerLoop::AuctionFulfillment => Some(BusinessEvent::Fulfillment),
        WorkerLoop::AuctionRecovery => Some(BusinessEvent::Recovery),
        WorkerLoop::OperationsAlertScan
        | WorkerLoop::OperationsAlertDispatch
        | WorkerLoop::OperationsAlertReconcile => Some(BusinessEvent::OperationsAlert),
    }
}

fn valid_retry_mode(loop_kind: WorkerLoop, retry_kind: WorkerRetryKind) -> bool {
    match loop_kind {
        WorkerLoop::MailMain | WorkerLoop::MailIsekai => true,
        WorkerLoop::Delivery | WorkerLoop::AuctionFulfillment | WorkerLoop::AuctionRecovery => {
            matches!(retry_kind, WorkerRetryKind::Persisted)
        }
        WorkerLoop::Auction
        | WorkerLoop::OperationsAlertScan
        | WorkerLoop::OperationsAlertDispatch
        | WorkerLoop::OperationsAlertReconcile => false,
    }
}

#[derive(Debug)]
struct DisabledTracingHealth;

impl TracingHealthReader for DisabledTracingHealth {
    fn snapshot(&self) -> TracingHealth {
        TracingHealth {
            status: TracingStatus::Disabled,
            dropped_span_count: 0,
            export_failure_count: 0,
            last_success_at: None,
            last_failure_at: None,
            last_error_code: String::new(),
        }
    }
}

/// 旧版 tracing collector 的低基数 Rust 实现。
///
/// counter 在读取时只向前同步到心跳安全快照，避免把不可信的负数或回退值写入 Prometheus。
struct TracingCollector {
    reader: Arc<dyn TracingHealthReader>,
    enabled: Gauge,
    degraded: Gauge,
    dropped_spans: IntCounter,
    export_failures: IntCounter,
    last_success: Gauge,
    last_failure: Gauge,
}

impl TracingCollector {
    fn new(reader: Arc<dyn TracingHealthReader>) -> Result<Self, MetricsError> {
        Ok(Self {
            reader,
            enabled: Gauge::with_opts(Opts::new(
                "auction_tracing_enabled",
                "Whether process tracing is enabled.",
            ))
            .map_err(MetricsError::Registry)?,
            degraded: Gauge::with_opts(Opts::new(
                "auction_tracing_export_degraded",
                "Whether tracing export is degraded.",
            ))
            .map_err(MetricsError::Registry)?,
            dropped_spans: IntCounter::with_opts(Opts::new(
                "auction_tracing_dropped_spans_total",
                "Total spans dropped by the bounded tracing processor.",
            ))
            .map_err(MetricsError::Registry)?,
            export_failures: IntCounter::with_opts(Opts::new(
                "auction_tracing_export_failures_total",
                "Total tracing export failures.",
            ))
            .map_err(MetricsError::Registry)?,
            last_success: Gauge::with_opts(Opts::new(
                "auction_tracing_export_last_success_timestamp_seconds",
                "Unix timestamp of the last successful tracing export, or zero.",
            ))
            .map_err(MetricsError::Registry)?,
            last_failure: Gauge::with_opts(Opts::new(
                "auction_tracing_export_last_failure_timestamp_seconds",
                "Unix timestamp of the last failed tracing export, or zero.",
            ))
            .map_err(MetricsError::Registry)?,
        })
    }

    fn sync_counter(counter: &IntCounter, value: i64) {
        let target = u64::try_from(value).unwrap_or(0);
        let current = counter.get();
        if target > current {
            counter.inc_by(target - current);
        }
    }

    fn timestamp_seconds(value: Option<chrono::DateTime<chrono::Utc>>) -> f64 {
        value.map_or(0.0, |timestamp| timestamp.timestamp_micros() as f64 / 1_000_000.0)
    }
}

impl Collector for TracingCollector {
    fn desc(&self) -> Vec<&Desc> {
        [
            self.enabled.desc(),
            self.degraded.desc(),
            self.dropped_spans.desc(),
            self.export_failures.desc(),
            self.last_success.desc(),
            self.last_failure.desc(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        let health = self.reader.snapshot();
        self.enabled.set(f64::from(!matches!(health.status, TracingStatus::Disabled)));
        self.degraded.set(f64::from(health.status == TracingStatus::Degraded));
        Self::sync_counter(&self.dropped_spans, health.dropped_span_count);
        Self::sync_counter(&self.export_failures, health.export_failure_count);
        self.last_success.set(Self::timestamp_seconds(health.last_success_at));
        self.last_failure.set(Self::timestamp_seconds(health.last_failure_at));

        [
            self.enabled.collect(),
            self.degraded.collect(),
            self.dropped_spans.collect(),
            self.export_failures.collect(),
            self.last_success.collect(),
            self.last_failure.collect(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_graphic() && byte != b'"')
}
