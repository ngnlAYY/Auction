//! 服务心跳与管理端健康快照的领域边界。
//!
//! 本模块只拥有安全的健康投影和存储窄端口，不读取配置、不打开网络监听，也不把进程
//! details 原文或 tracing exporter 细节传播到 HTTP。具体数据库实现位于
//! `auction-storage`，Admin 路由只消费 `HealthReader`。

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use thiserror::Error;

/// 心跳 details 的最大序列化大小，避免健康记录成为任意 JSON 存储通道。
pub const MAX_HEARTBEAT_DETAILS_BYTES: usize = 64 << 10;

/// 允许的心跳 TTL 上限。
pub const MAX_HEARTBEAT_TTL: Duration = Duration::hours(24);

/// 可观测性端口的异步返回值。
pub type HealthFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, HealthError>> + Send + 'a>>;

/// 心跳运行状态读取器的异步 readiness 返回值；失败依赖直接投影为 `false`，不泄漏错误细节。
pub type HeartbeatReadinessFuture<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// 健康存储或安全投影失败。
#[derive(Debug, Error)]
pub enum HealthError {
    #[error("健康命令字段无效")]
    InvalidCommand,

    #[error("健康数据投影无效")]
    InvalidData,

    #[error("心跳 observation sequence 已过期")]
    StaleObservation,

    #[error("健康存储不可用: {message}")]
    Storage { message: String },
}

/// 进程服务名称，与 `service_heartbeats` 的数据库闭集一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServiceName {
    Public,
    Admin,
    Worker,
}

impl ServiceName {
    /// 返回稳定的外部名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "auction-public",
            Self::Admin => "auction-admin",
            Self::Worker => "auction-worker",
        }
    }

    /// 解析数据库或协议中的服务名称。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auction-public" => Some(Self::Public),
            "auction-admin" => Some(Self::Admin),
            "auction-worker" => Some(Self::Worker),
            _ => None,
        }
    }

    const fn role(self) -> ProcessRole {
        match self {
            Self::Public => ProcessRole::Public,
            Self::Admin => ProcessRole::Admin,
            Self::Worker => ProcessRole::Worker,
        }
    }

    const ALL: [Self; 3] = [Self::Public, Self::Admin, Self::Worker];
}

/// 进程职责名称。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcessRole {
    Public,
    Admin,
    Worker,
}

impl ProcessRole {
    /// 返回稳定的外部名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
            Self::Worker => "worker",
        }
    }

    /// 解析数据库或协议中的职责名称。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "public" => Some(Self::Public),
            "admin" => Some(Self::Admin),
            "worker" => Some(Self::Worker),
            _ => None,
        }
    }
}

/// 心跳允许跨进程公开的进程状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartbeatStatus {
    Starting,
    Ready,
    Degraded,
    Stopping,
}

impl HeartbeatStatus {
    /// 返回稳定的外部名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Degraded => "degraded",
            Self::Stopping => "stopping",
        }
    }

    /// 解析数据库或协议中的状态名称。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "starting" => Some(Self::Starting),
            "ready" => Some(Self::Ready),
            "degraded" => Some(Self::Degraded),
            "stopping" => Some(Self::Stopping),
            _ => None,
        }
    }
}

/// tracing 健康状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracingStatus {
    Disabled,
    Ok,
    Degraded,
}

impl TracingStatus {
    /// 返回稳定的外部名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Ok => "ok",
            Self::Degraded => "degraded",
        }
    }

    /// 解析数据库或协议中的 tracing 状态。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "ok" => Some(Self::Ok),
            "degraded" => Some(Self::Degraded),
            _ => None,
        }
    }
}

/// 不包含 exporter 配置、原始错误或 span 内容的 tracing 安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracingHealth {
    pub status: TracingStatus,
    pub dropped_span_count: i64,
    pub export_failure_count: i64,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_error_code: String,
}

impl TracingHealth {
    /// 校验 tracing 计数、时间和错误码的组合，避免把损坏状态展示为健康。
    pub fn validate(&self) -> bool {
        if self.dropped_span_count < 0
            || self.export_failure_count < 0
            || self.last_success_at.is_some_and(|value| value.timestamp_micros() == 0)
            || self.last_failure_at.is_some_and(|value| value.timestamp_micros() == 0)
            || !matches!(
                self.last_error_code.as_str(),
                "" | "timeout" | "canceled" | "transport" | "rejected" | "shutdown"
            )
        {
            return false;
        }
        if self.status == TracingStatus::Disabled {
            return self.dropped_span_count == 0
                && self.export_failure_count == 0
                && self.last_success_at.is_none()
                && self.last_failure_at.is_none()
                && self.last_error_code.is_empty();
        }
        if self.export_failure_count == 0 && self.last_failure_at.is_some()
            || self.export_failure_count > 0 && self.last_failure_at.is_none()
            || !self.last_error_code.is_empty()
                && (self.status != TracingStatus::Degraded || self.last_failure_at.is_none())
            || self.status == TracingStatus::Degraded
                && self.dropped_span_count == 0
                && self.last_error_code.is_empty()
        {
            return false;
        }
        self.status != TracingStatus::Ok
            || self.last_failure_at.is_none()
            || self.last_success_at.is_some_and(|success| {
                self.last_failure_at.is_none_or(|failure| success >= failure)
            })
    }
}

/// 提供当前进程 tracing 安全状态的窄读取端口。
///
/// 心跳只读取计数、状态、时间和闭集错误码，不读取 exporter 配置、端点或 span 内容。
pub trait TracingHealthReader: Send + Sync {
    /// 返回当前 tracing 的安全快照。
    fn snapshot(&self) -> TracingHealth;
}

/// 提供心跳每轮所需的真实数据库/schema readiness 与进程内审计失败计数。
pub trait HeartbeatHealthReader: Send + Sync {
    /// 重新执行一次只读 readiness 检查；不可用或 schema 非 current 时返回 `false`。
    fn readiness(&self) -> HeartbeatReadinessFuture<'_>;

    /// 返回不包含 SQL、错误原文或业务数据的审计写入失败饱和计数。
    fn audit_failure_count(&self) -> i64;
}

/// 进程心跳写入命令。
#[derive(Debug, Clone)]
pub struct HeartbeatCommand {
    pub service_name: ServiceName,
    pub process_role: ProcessRole,
    pub instance_id: String,
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub status: HeartbeatStatus,
    pub readiness: bool,
    pub audit_failure_count: i64,
    pub tracing: TracingHealth,
    pub observation_sequence: i64,
    pub ttl: Duration,
    pub details: Value,
}

impl HeartbeatCommand {
    /// 校验身份闭集、递增序列、TTL 和安全 JSON 对象。
    pub fn validate(&self) -> bool {
        self.service_name.role() == self.process_role
            && valid_text(&self.instance_id, 128)
            && valid_text(&self.version, 128)
            && self.started_at.timestamp_micros() != 0
            && self.observation_sequence > 0
            && self.audit_failure_count >= 0
            && self.ttl > Duration::zero()
            && self.ttl <= MAX_HEARTBEAT_TTL
            && self.tracing.validate()
            && valid_details(&self.details)
    }
}

/// 心跳发布器的稳定进程身份与租约配置。
#[derive(Debug, Clone)]
pub struct HeartbeatPublisherConfig {
    pub service_name: ServiceName,
    pub process_role: ProcessRole,
    pub instance_id: String,
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub ttl: Duration,
}

/// 进程装配根使用的心跳发布器。
///
/// 发布器只负责生成有界的安全命令并分配本进程内的递增 observation sequence；周期调度、
/// 运行时和优雅关闭仍由各自的装配根负责，避免让可观测性 crate 反向拥有 Tokio 生命周期。
pub struct HeartbeatPublisher {
    store: Arc<dyn HeartbeatStore>,
    service_name: ServiceName,
    process_role: ProcessRole,
    instance_id: String,
    version: String,
    started_at: DateTime<Utc>,
    ttl: Duration,
    process_health: Arc<dyn HeartbeatHealthReader>,
    tracing_health: Arc<dyn TracingHealthReader>,
    observation_sequence: AtomicI64,
}

impl HeartbeatPublisher {
    /// 建立一个新的进程实例发布器；`instance_id` 必须在进程重启时变化。
    pub fn new(
        store: Arc<dyn HeartbeatStore>,
        config: HeartbeatPublisherConfig,
        process_health: Arc<dyn HeartbeatHealthReader>,
        tracing_health: Arc<dyn TracingHealthReader>,
    ) -> Self {
        Self {
            store,
            service_name: config.service_name,
            process_role: config.process_role,
            instance_id: config.instance_id,
            version: config.version,
            started_at: config.started_at,
            ttl: config.ttl,
            process_health,
            tracing_health,
            observation_sequence: AtomicI64::new(0),
        }
    }

    /// 发布指定状态的安全心跳；存储失败由调用方决定是否降级或继续运行。
    pub async fn publish(
        &self,
        status: HeartbeatStatus,
        readiness: bool,
        audit_failure_count: i64,
        tracing: TracingHealth,
    ) -> Result<(), HealthError> {
        let previous = self
            .observation_sequence
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| value.checked_add(1))
            .map_err(|_| HealthError::InvalidCommand)?;
        let observation_sequence = previous.checked_add(1).ok_or(HealthError::InvalidCommand)?;
        self.store
            .publish(HeartbeatCommand {
                service_name: self.service_name,
                process_role: self.process_role,
                instance_id: self.instance_id.clone(),
                version: self.version.clone(),
                started_at: self.started_at,
                status,
                readiness,
                audit_failure_count,
                tracing,
                observation_sequence,
                ttl: self.ttl,
                details: serde_json::json!({}),
            })
            .await
    }

    /// 发布启动中状态。
    pub async fn publish_starting(&self) -> Result<(), HealthError> {
        self.publish(
            HeartbeatStatus::Starting,
            false,
            self.process_health.audit_failure_count(),
            self.tracing_snapshot(),
        )
        .await
    }

    /// 重新读取依赖、审计和 tracing 状态并发布当前安全快照。
    pub async fn publish_current(&self) -> Result<(), HealthError> {
        let dependency_readiness = self.process_health.readiness().await;
        let audit_failure_count = self.process_health.audit_failure_count();
        let readiness = dependency_readiness && audit_failure_count == 0;
        let tracing = self.tracing_snapshot();
        let status = if readiness && tracing.status != TracingStatus::Degraded {
            HeartbeatStatus::Ready
        } else {
            HeartbeatStatus::Degraded
        };
        self.publish(status, readiness, audit_failure_count, tracing).await
    }

    /// 发布停止状态；停止状态仍然带有短 TTL，避免残留实例长期显示为活动进程。
    pub async fn publish_stopping(&self) -> Result<(), HealthError> {
        self.publish(
            HeartbeatStatus::Stopping,
            false,
            self.process_health.audit_failure_count(),
            self.tracing_snapshot(),
        )
        .await
    }

    fn tracing_snapshot(&self) -> TracingHealth {
        self.tracing_health.snapshot()
    }
}

/// 数据库中的可过期健康快照。
#[derive(Debug, Clone)]
pub struct Heartbeat {
    pub service_name: ServiceName,
    pub process_role: ProcessRole,
    pub instance_id: String,
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub status: HeartbeatStatus,
    pub readiness: bool,
    pub audit_failure_count: i64,
    pub tracing: TracingHealth,
    pub observation_sequence: i64,
    pub observed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl Heartbeat {
    /// 校验数据库返回的安全投影；details 不在 dashboard 快照中读取。
    pub fn validate(&self) -> bool {
        self.service_name.role() == self.process_role
            && valid_text(&self.instance_id, 128)
            && valid_text(&self.version, 128)
            && self.started_at.timestamp_micros() != 0
            && self.observation_sequence > 0
            && self.audit_failure_count >= 0
            && self.observed_at.timestamp_micros() != 0
            && self.expires_at > self.observed_at
            && self.observed_at >= self.started_at
            && self.tracing.validate()
    }
}

/// 三个进程的保守健康聚合。
#[derive(Debug, Clone)]
pub struct HealthSummary {
    pub status: HeartbeatStatus,
    pub readiness: bool,
    pub audit_failure_count: i64,
    pub missing_services: Vec<ServiceName>,
}

/// 进程 dashboard 的安全条目，不包含 heartbeat details。
#[derive(Debug, Clone)]
pub struct ProcessState {
    pub service_name: ServiceName,
    pub process_role: ProcessRole,
    pub instance_id: String,
    pub version: String,
    pub started_at: DateTime<Utc>,
    pub status: HeartbeatStatus,
    pub readiness: bool,
    pub audit_failure_count: i64,
    pub tracing: TracingHealth,
    pub observed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// 进程 dashboard 安全快照。
#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub processes: Vec<ProcessState>,
    pub summary: HealthSummary,
    pub observed_at: Option<DateTime<Utc>>,
    pub missing_services: Vec<ServiceName>,
}

/// dashboard section 的安全状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardStatus {
    Ok,
    Degraded,
    Unknown,
}

impl DashboardStatus {
    /// 返回稳定的 section 状态名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Degraded => "degraded",
            Self::Unknown => "unknown",
        }
    }
}

/// 可独立失败的 dashboard section。
#[derive(Debug, Clone)]
pub struct DashboardSection<T> {
    pub status: DashboardStatus,
    pub observed_at: Option<DateTime<Utc>>,
    pub error_code: Option<&'static str>,
    pub data: Option<T>,
}

/// 可观测性 owner 提供的 Admin dashboard 基础快照。
///
/// HTTP 层会在此基础上组合 mail、realtime、credentials、delivery、fulfillment、recovery
/// 和 alerting 等其他 owner 的安全投影；本结构只拥有健康与进程 section。
#[derive(Debug, Clone)]
pub struct AdminDashboardSnapshot {
    pub health: HealthSummary,
    pub process: DashboardSection<ProcessSnapshot>,
}

/// 心跳持久化窄端口。
pub trait HeartbeatStore: Send + Sync {
    /// 按 observation sequence 条件 upsert 心跳；旧序列必须失败关闭。
    fn publish(&self, command: HeartbeatCommand) -> HealthFuture<'_, ()>;

    /// 只返回尚未过期的心跳，过期判断由权威数据库完成。
    fn list_live(&self) -> HealthFuture<'_, Vec<Heartbeat>>;
}

/// Admin dashboard 读取窄端口。
pub trait HealthReader: Send + Sync {
    fn snapshot(&self) -> HealthFuture<'_, AdminDashboardSnapshot>;
}

/// 可观测性健康服务。
pub struct HealthService {
    store: Arc<dyn HeartbeatStore>,
}

impl HealthService {
    /// 创建使用显式心跳存储的服务。
    pub fn new(store: Arc<dyn HeartbeatStore>) -> Self {
        Self { store }
    }
}

impl HealthReader for HealthService {
    fn snapshot(&self) -> HealthFuture<'_, AdminDashboardSnapshot> {
        Box::pin(async move {
            let heartbeats = self.store.list_live().await?;
            if heartbeats.iter().any(|heartbeat| !heartbeat.validate()) {
                return Err(HealthError::InvalidData);
            }
            let health = summarize(&heartbeats);
            let processes = heartbeats.iter().map(process_state).collect::<Vec<_>>();
            let observed_at = processes.iter().map(|process| process.observed_at).max();
            let status = if health.status == HeartbeatStatus::Ready {
                DashboardStatus::Ok
            } else {
                DashboardStatus::Degraded
            };
            Ok(AdminDashboardSnapshot {
                health: health.clone(),
                process: DashboardSection {
                    status,
                    observed_at,
                    error_code: None,
                    data: Some(ProcessSnapshot {
                        processes,
                        missing_services: health.missing_services.clone(),
                        summary: health,
                        observed_at,
                    }),
                },
            })
        })
    }
}

fn summarize(heartbeats: &[Heartbeat]) -> HealthSummary {
    let mut seen = [false; 3];
    let mut operationally_healthy = true;
    let mut readiness = true;
    let mut audit_failure_count = 0_i64;
    for heartbeat in heartbeats {
        let index = match heartbeat.service_name {
            ServiceName::Public => 0,
            ServiceName::Admin => 1,
            ServiceName::Worker => 2,
        };
        seen[index] = true;
        if heartbeat.status != HeartbeatStatus::Ready
            || heartbeat.audit_failure_count > 0
            || heartbeat.tracing.status == TracingStatus::Degraded
        {
            operationally_healthy = false;
        }
        if !heartbeat.readiness
            || heartbeat.status == HeartbeatStatus::Starting
            || heartbeat.status == HeartbeatStatus::Stopping
            || heartbeat.audit_failure_count > 0
        {
            readiness = false;
        }
        audit_failure_count = audit_failure_count.saturating_add(heartbeat.audit_failure_count);
    }
    let missing_services = ServiceName::ALL
        .into_iter()
        .enumerate()
        .filter_map(|(index, service)| (!seen[index]).then_some(service))
        .collect::<Vec<_>>();
    if !missing_services.is_empty() {
        operationally_healthy = false;
        readiness = false;
    }
    HealthSummary {
        status: if operationally_healthy {
            HeartbeatStatus::Ready
        } else {
            HeartbeatStatus::Degraded
        },
        readiness,
        audit_failure_count,
        missing_services,
    }
}

fn process_state(heartbeat: &Heartbeat) -> ProcessState {
    ProcessState {
        service_name: heartbeat.service_name,
        process_role: heartbeat.process_role,
        instance_id: heartbeat.instance_id.clone(),
        version: heartbeat.version.clone(),
        started_at: heartbeat.started_at,
        status: heartbeat.status,
        readiness: heartbeat.readiness,
        audit_failure_count: heartbeat.audit_failure_count,
        tracing: heartbeat.tracing.clone(),
        observed_at: heartbeat.observed_at,
        expires_at: heartbeat.expires_at,
    }
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_details(value: &Value) -> bool {
    value.is_object() && value.to_string().len() <= MAX_HEARTBEAT_DETAILS_BYTES
}
