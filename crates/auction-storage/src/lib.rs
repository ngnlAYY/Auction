//! 数据库适配器边界的薄 crate 入口。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod database;
mod deployment_identity;

pub use database::*;

pub use deployment_identity::{
    DeploymentDatabaseIdentity, DeploymentDatabaseIdentityError,
    strict_deployment_database_identity,
};

pub use deployment_identity::{PostgresTarget, postgres_connect_options};
