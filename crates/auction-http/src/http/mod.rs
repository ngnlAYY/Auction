//! Public/Admin HTTP surface 的模块边界。
//!
//! `routes/` 拥有路由装配、handler、契约映射和共享 HTTP 辅助逻辑；本入口只负责把稳定
//! surface 与共享 listener/state 类型导出给 crate 根，避免 `mod.rs` 继续承担实现细节。

mod routes;

pub use routes::{
    AdminApiState, AdminHttpSecurity, HttpBoundary, HttpSecurityOptions, PublicApiState,
    PublicHttpSecurity, RealtimeHub, RealtimeHubHealth, RealtimeHubSink, RealtimeSubscription,
    admin_router, apply_browser_security_headers, public_router, serve_http,
};

#[cfg(target_os = "linux")]
pub use routes::{UnixHttpListener, UnixListenerOptions};

pub(crate) use routes::request_id;
