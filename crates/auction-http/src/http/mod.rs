//! Public/Admin HTTP surface 的模块边界。
//! `admin/` 与 `public/` 按 surface 分组拥有 handler 与契约映射；`routes/` 拥有路由装配、
//! 共享 HTTP 辅助、安全与指标；本入口只负责把稳定 surface 与共享 listener/state 类型
//! 导出给 crate 根，避免 `mod.rs` 继续承担实现细节。

mod admin;
mod public;
mod readiness;
mod routes;

pub use readiness::HttpReadinessProbe;

pub use routes::{
    AdminApiState, AdminHttpSecurity, AnonymousReadGate, HttpBoundary, HttpSecurityOptions,
    PublicApiState, PublicHttpSecurity, RealtimeHub, RealtimeHubHealth, RealtimeHubSink,
    RealtimeSubscription, RealtimeSubscriptionError, admin_router, apply_browser_security_headers,
    public_router, serve_http, serve_metrics,
};

#[cfg(target_os = "linux")]
pub use routes::{UnixHttpListener, UnixListenerOptions};

pub(crate) use routes::request_id;
