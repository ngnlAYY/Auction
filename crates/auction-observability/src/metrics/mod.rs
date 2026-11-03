//! 进程本地 Prometheus 指标的分组入口；按进程、业务、HTTP、池、Worker 与 tracing owner 拆分。

mod audit;
mod business;
mod database;
pub use database::*;
mod http;
mod migration;
mod pool;
mod process;
mod readiness;
mod sent_baseline;
mod shutdown;
pub use shutdown::{
    ShutdownCriticalKind, ShutdownOutcome, ShutdownPhase, ShutdownReason, ShutdownRecorder,
    ShutdownRole,
};
mod tracing;
mod types;
mod worker;

pub use audit::{
    AuditFailureClass, AuditFailureKind, AuditFailureObservation, AuditMetricsSnapshot,
    AuditMetricsSource, AuditSqlstateClass,
};
pub use http::{HttpRateLimitKind, HttpRecorder, WebSocketConnectionGuard};
pub use migration::{MigrationReadinessBranch, MigrationReadinessRecorder};
pub use process::ProcessMetrics;
pub use readiness::{ReadinessCacheOutcome, ReadinessCheckResult, ReadinessRecorder};
pub use types::*;

mod runtime_switches;
