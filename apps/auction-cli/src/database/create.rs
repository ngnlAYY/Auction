//! 首次部署的建库：只在目标缺失时 CREATE DATABASE，已存在则只核对 owner 与应用连接；不迁移、不删库。

use std::path::Path;

use anyhow::Context;
use auction_storage::{PostgresBootstrapResult, PostgresDatabaseBootstrap};

use super::DatabaseTarget;
use crate::app::print_json;
use crate::cli::MaintenanceArgs;
use crate::deployment::ConfigDir;

pub(super) async fn run(
    config_dir: &ConfigDir,
    database: &str,
    maintenance_secrets: &Path,
    maintenance: &MaintenanceArgs,
) -> anyhow::Result<()> {
    let guard = crate::service::acquire_maintenance(config_dir, maintenance).await?;
    let config = config_dir.load_worker()?;
    let application_url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let application = DatabaseTarget::parse(application_url)?;
    application.require_database(database)?;
    let secrets = auction_config::load_postgres_maintenance_secrets(maintenance_secrets)
        .context("读取维护凭据失败")?;
    let maintenance_target = DatabaseTarget::parse(secrets.database_url())?;
    application.require_same_endpoint(&maintenance_target)?;
    if maintenance_target.database != "postgres" {
        anyhow::bail!("维护凭据的 [database].name 必须是 postgres");
    }
    let mut bootstrap = PostgresDatabaseBootstrap::connect(
        secrets.database_url(),
        &application.database,
        &application.username,
    )
    .await
    .context("连接维护库并取得建库锁失败")?;
    let result = bootstrap.create_or_verify(application_url).await.context("建库失败")?;
    let (outcome, database_name, owner) = match result {
        PostgresBootstrapResult::Created { database_name, owner } => {
            ("created", database_name, owner)
        }
        PostgresBootstrapResult::AlreadyExists { database_name, owner } => {
            ("already_exists", database_name, owner)
        }
    };
    print_json(&serde_json::json!({
        "guard": guard.mode,
        "outcome": outcome,
        "database": database_name,
        "owner": owner,
        "next_step": "auction-cli db migrate",
    }))
}
