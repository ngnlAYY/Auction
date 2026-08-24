//! HTTP 表面适配器的薄 crate 入口。

mod assets;
mod http;

pub use http::{
    AdminApiState, AdminHttpSecurity, PublicApiState, PublicHttpSecurity, RealtimeHub,
    RealtimeHubHealth, RealtimeHubSink, RealtimeSubscription, admin_router, public_router,
};
