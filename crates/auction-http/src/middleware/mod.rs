//! 请求边界：安全响应头、转发来源拒绝、限流读闸、严格 JSON 与响应缓存。

pub(crate) mod boundary;
pub(crate) mod method_strict;
pub(crate) mod read_gate;
pub(crate) mod response_cache;
pub(crate) mod security;
pub(crate) mod strict_json;

pub use boundary::HttpBoundary;
pub use security::{
    AdminHttpSecurity, AnonymousReadGate, HttpSecurityOptions, PublicHttpSecurity,
    apply_browser_security_headers,
};
