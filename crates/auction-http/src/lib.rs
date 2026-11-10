//! HTTP 表面适配器的薄 crate 入口。
//!
//! `admin/` 与 `public/` 按 surface 分组拥有 handler 与路由装配；`middleware/` 拥有请求
//! 边界；`server/` 拥有监听、连接服务与指标出口；`mapping/`、`session/`、`state/`、
//! `reads/`、`realtime_hub/` 持有共享的 DTO 映射、会话、状态与推送能力。

mod admin;
mod assets;
mod diagnostics;
mod mapping;
mod middleware;
mod public;
mod reads;
mod realtime_hub;
mod server;
mod session;
mod state;

pub use assets::serve_admin_setup_asset;
pub use middleware::{
    AdminHttpSecurity, AnonymousReadGate, HttpBoundary, HttpSecurityOptions, PublicHttpSecurity,
    apply_browser_security_headers,
};
pub use realtime_hub::{
    RealtimeHub, RealtimeHubHealth, RealtimeHubSink, RealtimeSubscription,
    RealtimeSubscriptionError,
};
pub use server::{
    HttpListener, HttpReadinessProbe, admin_router, public_router, serve_http, serve_metrics,
    spawn_metrics_listener,
};
pub use state::{AdminApiState, PublicApiState};

#[cfg(target_os = "linux")]
pub use server::{UnixHttpListener, UnixListenerOptions};
