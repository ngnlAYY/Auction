//! 可观测性装配的薄 crate 入口。

mod health;
mod metrics;
mod telemetry;

pub use health::{
    AdminDashboardSnapshot, DashboardSection, DashboardStatus, HealthError, HealthFuture,
    HealthReader, HealthService, HealthSummary, Heartbeat, HeartbeatCommand, HeartbeatPublisher,
    HeartbeatStatus, HeartbeatStore, ProcessRole, ProcessSnapshot, ProcessState, ServiceName,
    TracingHealth, TracingHealthReader, TracingStatus,
};
pub use metrics::{
    BusinessEvent, BusinessOutcome, BusinessRecorder, METRICS_ROUTE, MetricsError,
    PoolMetricsBackend, PoolMetricsSnapshot, PoolMetricsSource, ProcessMetrics, WorkerLoop,
    WorkerRecorder, WorkerResult, WorkerRetryKind, noop_worker_recorder,
};
pub use telemetry::{
    InitError, ShutdownError, TracingConfig, TracingRuntime, init, init_with_config,
};
