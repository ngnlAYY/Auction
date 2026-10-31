//! 数据库适配器边界的薄 crate 入口。

mod database;
mod deployment_identity;

pub use database::*;

pub use deployment_identity::{
    DeploymentDatabaseIdentity, DeploymentDatabaseIdentityError, deployment_database_identity,
    strict_deployment_database_identity,
};

#[cfg(feature = "postgres")]
pub use deployment_identity::postgres_connect_options;
