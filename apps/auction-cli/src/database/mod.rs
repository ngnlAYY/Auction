//! 数据库运维命令：所有连接都来自 Worker 配置的敏感 [database]，连接串不进入参数、日志或输出。

use anyhow::Context;
use auction_storage::{DatabaseStore, PostgresRecoveryPreflight};
use serde::Serialize;

use crate::app::{print_json, progress};
use crate::cli::DbCommand;
use crate::deployment::ConfigDir;

mod create;
mod evidence;
mod rebuild;
mod target;

pub(crate) use target::{DatabaseTarget, decode_component};

pub(crate) async fn run(config_dir: &ConfigDir, command: DbCommand) -> anyhow::Result<()> {
    match command {
        DbCommand::Status => status(config_dir).await,
        DbCommand::Migrate { maintenance } => {
            let guard = crate::service::acquire_maintenance(config_dir, &maintenance).await?;
            let config = config_dir.load_worker()?;
            let url = config.database_url().context("Worker 配置缺少数据库连接")?;
            let report = apply_migrations(url).await?;
            print_json(&serde_json::json!({ "guard": guard.mode, "migration": report }))
        }
        DbCommand::Create { database, maintenance_secrets, maintenance } => {
            create::run(config_dir, &database, &maintenance_secrets, &maintenance).await
        }
        DbCommand::Preflight { database, output } => {
            evidence::preflight(config_dir, &database, &output).await
        }
        DbCommand::Inventory { database, output } => {
            evidence::inventory(config_dir, &database, &output).await
        }
        DbCommand::Rebuild {
            database,
            maintenance_secrets,
            mode,
            backup_manifest,
            ephemeral,
            execute,
            confirm,
            receipt,
            maintenance,
        } => {
            rebuild::run(rebuild::Options {
                config_dir,
                database: &database,
                maintenance_secrets: &maintenance_secrets,
                mode,
                backup_manifest: backup_manifest.as_deref(),
                ephemeral,
                execute,
                confirm: confirm.as_deref(),
                receipt: receipt.as_deref(),
                maintenance: &maintenance,
            })
            .await
        }
    }
}

/// 连接应用数据库；调用方负责在结束前关闭连接池。
pub(crate) async fn connect(url: &str) -> anyhow::Result<DatabaseStore> {
    DatabaseStore::connect(url).await.context("连接 PostgreSQL 失败")
}

/// 只读读取目标身份与迁移状态；history 不是完整成功前缀时失败。
pub(crate) async fn preflight(
    store: &DatabaseStore,
    database: &str,
) -> anyhow::Result<PostgresRecoveryPreflight> {
    let DatabaseStore::Postgres(postgres) = store;
    postgres.collect_recovery_preflight(database).await.context("读取数据库身份与迁移历史失败")
}

#[derive(Debug, Serialize)]
pub(crate) struct MigrationReport {
    database: String,
    before_version: i64,
    after_version: i64,
    expected_version: i64,
    applied: i64,
}

/// 在 SQLx 迁移锁内执行全部待执行迁移；空库会建立完整 schema，未知非空库被拒绝。
pub(crate) async fn apply_migrations(url: &str) -> anyhow::Result<MigrationReport> {
    let target = DatabaseTarget::parse(url)?;
    let store = connect(url).await?;
    let result = async {
        let before = latest_version(&preflight(&store, &target.database).await?);
        progress(format_args!("迁移 {}：当前版本 {before}", target.database));
        store.migrate().await.context("执行数据库迁移失败")?;
        let after = latest_version(&preflight(&store, &target.database).await?);
        Ok(MigrationReport {
            database: target.database.clone(),
            before_version: before,
            after_version: after,
            expected_version: auction_storage::postgres_expected_migration_version(),
            applied: after - before,
        })
    }
    .await;
    store.postgres_pool().close().await;
    result
}

fn latest_version(preflight: &PostgresRecoveryPreflight) -> i64 {
    preflight.migration.as_ref().map_or(0, |migration| migration.latest_version)
}

async fn status(config_dir: &ConfigDir) -> anyhow::Result<()> {
    let config = config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let target = DatabaseTarget::parse(url)?;
    let store = connect(url).await?;
    let result = preflight(&store, &target.database).await;
    store.postgres_pool().close().await;
    let preflight = result?;
    let expected = auction_storage::postgres_expected_migration_version();
    let current = preflight.migration.as_ref().is_some_and(|migration| migration.current);
    print_json(&serde_json::json!({
        "database": preflight.database_name,
        "server_version": preflight.server_version,
        "in_recovery": preflight.in_recovery,
        "default_transaction_read_only": preflight.default_transaction_read_only,
        "active_other_sessions": preflight.active_other_sessions,
        "database_size_bytes": preflight.database_size_bytes,
        "schema_identity": preflight.schema_identity,
        "migration": preflight.migration,
        "expected_version": expected,
        "schema_current": current,
    }))?;
    if current {
        Ok(())
    } else {
        anyhow::bail!("数据库 schema 不是当前版本 {expected}；停机后执行 auction-cli db migrate")
    }
}
