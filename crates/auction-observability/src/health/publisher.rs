use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use super::model::{
    HealthError, HealthFuture, HeartbeatReadinessFuture, HeartbeatStatus, ProcessRole, ServiceName,
    TracingHealth, TracingStatus, valid_details, valid_text,
};

/// 提供当前进程 tracing 安全状态的窄读取端口。
///
/// 心跳只读取计数、状态、时间和闭集错误码，不读取 exporter 配置、端点或 span 内容。
pub trait TracingHealthReader: Send + Sync {
    /// 返回当前 tracing 的安全快照。
    fn snapshot(&self) -> TracingHealth;
}

/// 提供心跳每轮所需的真实数据库/schema readiness 与进程内审计失败计数。
pub trait HeartbeatHealthReader: Send + Sync {
    /// 重新执行一次只读 readiness 检查；不可用或 schema 非 current 时返回 false。
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
            && self.ttl <= super::model::MAX_HEARTBEAT_TTL
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
    /// 建立一个新的进程实例发布器；instance_id 必须在进程重启时变化。
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

/// 心跳持久化窄端口。
pub trait HeartbeatStore: Send + Sync {
    /// 按 observation sequence 条件 upsert 心跳；旧序列必须失败关闭。
    fn publish(&self, command: HeartbeatCommand) -> HealthFuture<'_, ()>;

    /// 只返回尚未过期的心跳，过期判断由权威数据库完成。
    fn list_live(&self) -> HealthFuture<'_, Vec<Heartbeat>>;
}
