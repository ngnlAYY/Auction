//! HTTP 表面适配器的薄 crate 入口。
//!
//! `admin/` 与 `public/` 按 surface 分组拥有 handler；`middleware/` 拥有请求边界；`server/`
//! 拥有路由表、监听、连接服务与指标出口；`errors/`、`query/`、`mapping/` 持有共享的错误
//! 外壳、查询解析与时间戳/游标编码；`session/`、`state/`、`reads/`、`realtime_hub/` 持有
//! 会话、状态、Public 只读处理器与推送能力。

mod admin;
mod assets;
mod diagnostics;
mod errors;
mod mapping;
mod middleware;
mod public;
mod query;
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
pub use realtime_hub::{RealtimeHub, RealtimeHubSink, RealtimeSubscriptionError};
pub use server::{
    HttpListener, HttpReadinessProbe, admin_router, public_router, serve_http, serve_metrics,
    spawn_metrics_listener,
};
pub use state::{AdminApiState, PublicApiState};

#[cfg(target_os = "linux")]
pub use server::{UnixHttpListener, UnixListenerOptions};
