//! 可观测性装配的薄 crate 入口。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod health;
mod logging;
mod metrics;
pub use metrics::{RealtimeRecorder, SliMetrics};
mod telemetry;

pub use health::{
    AdminDashboardSnapshot, DashboardSection, DashboardStatus, HealthError, HealthFuture,
    HealthReader, HealthService, HealthSummary, Heartbeat, HeartbeatCommand, HeartbeatHealthReader,
    HeartbeatPublisher, HeartbeatPublisherConfig, HeartbeatReadinessFuture, HeartbeatStatus,
    HeartbeatStore, ProcessRole, ProcessSnapshot, ProcessState, ServiceName, TracingHealth,
    TracingHealthReader, TracingStatus,
};
pub use logging::{
    DiagnosticLogOwner, DiagnosticLogWindows, RateLimitedLog, install_safe_panic_hook,
};
pub use metrics::{
    AuditFailureClass, AuditFailureKind, AuditFailureObservation, AuditMetricsSnapshot,
    AuditMetricsSource, AuditSqlstateClass, BidQueueOutcome, BidQueueWaitStage, BrowserErrorClass,
    BusinessEvent, BusinessOutcome, BusinessRecorder, DatabaseAcquireOutcome, DatabaseConflictSite,
    DatabaseConnectionGuard, DatabaseRecorder, DatabaseReservedPurpose, DatabaseTransactionOwner,
    DatabaseTransactionPhase, HttpRateLimitKind, HttpRecorder, METRICS_ROUTE, MetricsError,
    MigrationReadinessBranch, MigrationReadinessRecorder, PoolMetricsBackend, PoolMetricsSnapshot,
    PoolMetricsSource, ProcessMetrics, ReadinessCacheOutcome, ReadinessCheckResult,
    ReadinessRecorder, ShutdownCriticalKind, ShutdownOutcome, ShutdownPhase, ShutdownReason,
    ShutdownRecorder, ShutdownRole, WebSocketConnectionGuard, WorkerLoop, WorkerPanicOwner,
    WorkerRecorder, WorkerResult, WorkerRetryKind, current_database_worker_loop,
    database_transaction_attempt, database_transaction_exhausted, database_transaction_phase,
    noop_worker_recorder, record_database_conflict, scope_database_worker_loop,
};
pub use telemetry::{
    InitError, ShutdownError, TracingConfig, TracingRuntime, attach_remote_trace_context,
    init_json_stderr, init_with_config, shutdown_tracing,
};
