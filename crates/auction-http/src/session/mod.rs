//! HTTP 会话校验、Cookie、认证错误和业务错误映射的模块装配。
//! Public/Admin handler 经本文件统一引用；实现按安全边界拆入限流认证、错误映射、Cookie、
//! 响应和指标子模块。

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
