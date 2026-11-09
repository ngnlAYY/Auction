//! HTTP 表面适配器的薄 crate 入口。

mod assets;
mod diagnostics;
mod http;

pub use assets::serve_admin_setup_asset;
pub use http::{
    AdminApiState, AdminHttpSecurity, AnonymousReadGate, HttpBoundary, HttpListener,
    HttpReadinessProbe, HttpSecurityOptions, PublicApiState, PublicHttpSecurity, RealtimeHub,
    RealtimeHubHealth, RealtimeHubSink, RealtimeSubscription, RealtimeSubscriptionError,
    admin_router, apply_browser_security_headers, public_router, serve_http, serve_metrics,
    spawn_metrics_listener,
};

#[cfg(target_os = "linux")]
pub use http::{UnixHttpListener, UnixListenerOptions};
