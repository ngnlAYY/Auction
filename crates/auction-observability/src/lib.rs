//! 可观测性装配的薄 crate 入口。

mod health;
mod logging;
mod metrics;
mod telemetry;

pub use health::{
    AdminDashboardSnapshot, DashboardSection, DashboardStatus, HealthError, HealthFuture,
    HealthReader, HealthService, HealthSummary, Heartbeat, HeartbeatCommand, HeartbeatHealthReader,
    HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatReadinessFuture, HeartbeatStatus,
    HeartbeatStore, ProcessRole, ProcessSnapshot, ProcessState, ServiceName, TracingHealth,
    TracingHealthReader, TracingStatus,
};
pub use logging::{DiagnosticLogWindows, RateLimitedLog, install_safe_panic_hook};
pub use metrics::{
    AuditFailureClass, AuditFailureKind, AuditFailureObservation, AuditMetricsSnapshot,
    AuditMetricsSource, AuditSqlstateClass, BusinessEvent, BusinessOutcome, BusinessRecorder,
    HttpRateLimitKind, HttpRecorder, METRICS_ROUTE, MetricsError, MigrationReadinessBranch,
    MigrationReadinessRecorder, PoolMetricsBackend, PoolMetricsSnapshot, PoolMetricsSource,
    ProcessMetrics, ReadinessCacheOutcome, ReadinessCheckResult, ReadinessRecorder,
    ShutdownCriticalKind, ShutdownOutcome, ShutdownPhase, ShutdownReason, ShutdownRecorder,
    ShutdownRole, WebSocketConnectionGuard, WorkerLoop, WorkerRecorder, WorkerResult,
    WorkerRetryKind, noop_worker_recorder,
};
pub use telemetry::{
    InitError, ShutdownError, TracingConfig, TracingRuntime, attach_remote_trace_context,
    init_json_stderr, init_with_config,
};
