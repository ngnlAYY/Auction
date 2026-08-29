//! PostgreSQL 初始数据库 bootstrap 适配器。
//!
//! 该模块只拥有“目标库缺失时创建”的窄边界，不运行迁移、不删除数据库，也不修改已存在的
//! 数据库。`auctionctl` 在进入这里前负责配置来源、人工确认、维护 URL 和部署停机门禁。

mod guard;
mod model;
mod sql;

pub use guard::PostgresDatabaseBootstrap;
pub use model::{PostgresBootstrapResult, PostgresDatabaseBootstrapError};
