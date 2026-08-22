//! 可观测性装配的薄 crate 入口。

mod health;
mod telemetry;

pub use health::{
    AdminDashboardSnapshot, DashboardSection, DashboardStatus, HealthError, HealthFuture,
    HealthReader, HealthService, HealthSummary, Heartbeat, HeartbeatCommand, HeartbeatPublisher,
    HeartbeatStatus, HeartbeatStore, ProcessRole, ProcessSnapshot, ProcessState, ServiceName,
    TracingHealth, TracingStatus,
};
pub use telemetry::{InitError, init};
