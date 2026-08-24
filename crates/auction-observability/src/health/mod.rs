//! 服务心跳与管理端健康快照的领域边界。
//!
//! 本模块只拥有安全的健康投影和存储窄端口，不读取配置、不打开网络监听，也不把进程
//! details 原文或 tracing exporter 细节传播到 HTTP。具体数据库实现位于
//! auction-storage，Admin 路由只消费 HealthReader。

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
