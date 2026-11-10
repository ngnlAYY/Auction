//! Public 与 Admin 的路由装配、共享 HTTP 辅助与安全边界。
//! handler 按 surface 分组在 `http::admin` 与 `http::public`；本模块拥有路由表、会话、
//! 读接口 DTO 映射、请求边界、安全/指标/实时 Hub，并向两端的 handler 转出共同依赖的
//! 错误、会话与映射辅助。竞价、结算和状态机规则留在领域与应用层。

mod admin_alerts;
mod boundary;
mod browser_errors;
pub(crate) mod build_identity;
mod method_strict;
mod metrics;
mod read_gate;
pub(crate) mod realtime_hub;
mod response_cache;
mod security;
mod server;
mod state;
pub(crate) mod strict_json;
#[cfg(target_os = "linux")]
mod unix_listener;

mod admin;
mod health;
mod mapping;
mod public;
mod reads;
mod session;
mod surfaces;

pub(crate) use admin::*;
pub(crate) use mapping::*;
pub(crate) use public::*;
pub(crate) use reads::*;
pub(crate) use session::*;

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

/// 解析旧 Public v1 使用的规范小写、非零 UUID。
pub(crate) fn parse_public_uuid(value: &str) -> Result<Uuid, ()> {
    parse_canonical_uuid(value).ok_or(())
}

/// Public 与严格 Admin 路由共用规范 UUID 校验；格式化只使用栈缓冲。
pub(crate) fn parse_canonical_uuid(value: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(value).ok()?;
    if id.is_nil() {
        return None;
    }
    let mut buffer = Uuid::encode_buffer();
    let canonical: &str = id.hyphenated().encode_lower(&mut buffer);
    (canonical == value).then_some(id)
}

pub use boundary::HttpBoundary;
pub use metrics::{serve_metrics, spawn_metrics_listener};
pub use realtime_hub::{
    RealtimeHub, RealtimeHubHealth, RealtimeHubSink, RealtimeSubscription,
    RealtimeSubscriptionError,
};
pub use security::{
    AdminHttpSecurity, AnonymousReadGate, HttpSecurityOptions, PublicHttpSecurity,
    apply_browser_security_headers,
};
pub use server::{HttpListener, serve_http};
pub use state::{AdminApiState, PublicApiState};
pub use surfaces::{admin_router, public_router};
#[cfg(target_os = "linux")]
pub use unix_listener::{UnixHttpListener, UnixListenerOptions};

mod admin_runtime_switches;
