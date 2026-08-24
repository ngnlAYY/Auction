//! Admin 凭据值对象的稳定出口。
//!
//! 闭集模型、明文/密文安全值、Worker 运行记录、dashboard 聚合和业务错误分别由子模块
//! 持有；本文件只负责装配与稳定导出。

mod common;
mod error;
mod health;
mod model;
mod runtime;
mod secret;

pub use common::{
    MAX_ADMIN_CREDENTIAL_OFFSET, MAX_ADMIN_CREDENTIAL_PAGE_SIZE, MAX_ADMIN_CREDENTIAL_SECRET_BYTES,
};
pub use error::*;
pub use health::*;
pub use model::*;
pub use runtime::*;
pub use secret::*;
