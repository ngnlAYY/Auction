//! 服务心跳与管理端健康快照的领域边界；只拥有安全投影和存储窄端口，不读配置、不监听，
//! 数据库实现在 auction-storage。

mod dashboard;
mod model;
mod publisher;

pub use dashboard::{
    AdminDashboardSnapshot, DashboardSection, DashboardStatus, HealthReader, HealthService,
    HealthSummary, ProcessSnapshot, ProcessState,
};
pub use model::{
    HealthError, HealthFuture, HeartbeatReadinessFuture, HeartbeatStatus, ProcessRole, ServiceName,
    TracingHealth, TracingStatus,
};
pub use publisher::{
    Heartbeat, HeartbeatCommand, HeartbeatHealthReader, HeartbeatPublisher,
    HeartbeatPublisherConfig, HeartbeatStore, TracingHealthReader,
};
