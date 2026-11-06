//! HTTP 会话校验、Cookie、认证错误和业务错误映射的模块装配。
//! Public/Admin 路由通过本文件保留既有 `routes::session::*` 出口；实现按安全边界拆入
//! 限流认证、错误映射、Cookie、响应和指标子模块，避免改变 HTTP 契约。

mod cookies;
mod errors;
mod guards;
mod metrics;
mod responses;

pub(crate) use cookies::*;
pub(crate) use errors::*;
pub(crate) use guards::*;
pub(crate) use metrics::*;
pub(crate) use responses::*;
