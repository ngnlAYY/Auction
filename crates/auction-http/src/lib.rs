//! HTTP 表面适配器的薄 crate 入口。

mod assets;
mod http;

pub use assets::serve_admin_setup_asset;
pub use http::{
    AdminApiState, AdminHttpSecurity, HttpBoundary, HttpSecurityOptions, PublicApiState,
    PublicHttpSecurity, RealtimeHub, RealtimeHubHealth, RealtimeHubSink, RealtimeSubscription,
    admin_router, apply_browser_security_headers, public_router, serve_http,
};

#[cfg(target_os = "linux")]
pub use http::{UnixHttpListener, UnixListenerOptions};
