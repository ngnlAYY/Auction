//! 部署数据库身份预检；与合并入口共用存储层的显式目标校验。

use anyhow::Context;
use auction_storage::{DeploymentDatabaseIdentity, strict_deployment_database_identity};

pub(super) fn database_identity(database_url: &str) -> anyhow::Result<DeploymentDatabaseIdentity> {
    strict_deployment_database_identity(database_url).context("确认部署数据库目标失败")
}
