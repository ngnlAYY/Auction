use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{
    model::{
        HealthError, HealthFuture, HeartbeatStatus, ProcessRole, ServiceName, TracingHealth,
        TracingStatus,
    },
    publisher::{Heartbeat, HeartbeatStore},
};

/// 三个进程的保守健康聚合。
#[derive(Debug, Clone)]
pub struct HealthSummary {
    pub status: HeartbeatStatus,
    pub readiness: bool,
    /// 各发布进程按模式投影的累计值之和；不用于反推全部瞬时失败数量。
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
    /// legacy/shadow 为全部失败；classified 仅为阻断就绪的失败。
    pub audit_failure_count: i64,
    pub tracing: TracingHealth,
    pub observed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

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
/// HTTP 层会在此基础上组合 mail、realtime、credentials、delivery、fulfillment、recovery
/// 和 alerting 等其他 owner 的安全投影；本结构只拥有健康与进程 section。
#[derive(Debug, Clone)]
pub struct AdminDashboardSnapshot {
    pub health: HealthSummary,
    pub process: DashboardSection<ProcessSnapshot>,
}

/// Admin dashboard 读取窄端口。
pub trait HealthReader: Send + Sync {
    fn snapshot(&self) -> HealthFuture<'_, AdminDashboardSnapshot>;
}

pub struct HealthService {
    store: Arc<dyn HeartbeatStore>,
}

impl HealthService {
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
            let processes = heartbeats.into_iter().map(process_state).collect::<Vec<_>>();
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

fn process_state(heartbeat: Heartbeat) -> ProcessState {
    ProcessState {
        service_name: heartbeat.service_name,
        process_role: heartbeat.process_role,
        instance_id: heartbeat.instance_id,
        version: heartbeat.version,
        started_at: heartbeat.started_at,
        status: heartbeat.status,
        readiness: heartbeat.readiness,
        audit_failure_count: heartbeat.audit_failure_count,
        tracing: heartbeat.tracing,
        observed_at: heartbeat.observed_at,
        expires_at: heartbeat.expires_at,
    }
}
