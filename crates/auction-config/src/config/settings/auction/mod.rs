//! 拍卖与进程通用配置模型的稳定出口。
//!
//! 监听、缓存、拍卖策略和可观测性配置分别归属相邻 owner；本文件只负责模块装配与稳定
//! 公共导出。

mod cache;
mod http;
mod observability;
mod policy;
mod role;

pub use cache::*;
pub use http::*;
pub use observability::*;
pub use policy::*;
pub use role::*;
