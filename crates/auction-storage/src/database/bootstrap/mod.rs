//! PostgreSQL 初始数据库 bootstrap 适配器；只拥有“目标库缺失时创建”的窄边界，不跑迁移、
//! 不删库、不改动已存在数据库，配置来源与停机门禁由 `auctionctl` 在进入前负责。

mod guard;
mod model;
mod sql;

pub use guard::PostgresDatabaseBootstrap;
pub use model::{PostgresBootstrapResult, PostgresDatabaseBootstrapError};
