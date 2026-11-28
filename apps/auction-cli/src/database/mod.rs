//! 数据库运维命令：只读与稳态连接来自 Worker 配置的敏感 `[database]`；需要 DDL 的建库、迁移、
//! 重建与恢复改由独立 owner-only 敏感 TOML 提供迁移 owner。连接串不进入参数、日志或输出。

use anyhow::Context;
use auction_storage::{DatabaseStore, PostgresRecoveryPreflight};
use serde::Serialize;

use crate::app::{print_json, progress};
use crate::cli::DbCommand;
use crate::config_dir::ConfigDir;

mod assets;
mod create;
mod evidence;
mod inspection;
pub(crate) mod profile;
mod purge;
mod rebuild;
mod target;

pub(crate) use target::{DatabaseTarget, decode_component};

pub(crate) async fn run(config_dir: &ConfigDir, command: DbCommand) -> anyhow::Result<()> {
    match command {
        DbCommand::Status => status(config_dir).await,
        DbCommand::Migrate { owner, maintenance } => {
            let guard = crate::service::acquire_maintenance(config_dir, &maintenance).await?;
            let config = config_dir.load_worker()?;
            let owner = load_owner_secrets(&owner.owner_secrets)?;
            let report = apply_migrations(&config, &owner).await?;
            print_json(&serde_json::json!({ "guard": guard.mode, "migration": report }))
        }
        DbCommand::Grants { owner, maintenance } => {
            let guard = crate::service::acquire_maintenance(config_dir, &maintenance).await?;
            let config = config_dir.load_worker()?;
            let owner = load_owner_secrets(&owner.owner_secrets)?;
            require_owner_target(&config, &owner)?;
            let store = connect_url(
                &config,
                owner.database_url(),
                auction_storage::DatabaseConnectionPurpose::CliMigrate,
            )
            .await?;
            closing(store, async |store| {
                let present = auction_storage::runtime_privileges_present(store.postgres_pool())
                    .await
                    .context("重放运行账号最小权限失败")?;
                if !present {
                    anyhow::bail!(
                        "数据库缺少运行账号授权函数；先执行 auction-cli db migrate --owner-secrets <file>"
                    );
                }
                auction_storage::apply_runtime_privileges(store.postgres_pool())
                    .await
                    .context("重放运行账号最小权限失败")
            })
            .await?;
            print_json(&serde_json::json!({
                "guard": guard.mode,
                "outcome": "privileges_applied",
                "runtime_role": auction_config::RUNTIME_DATABASE_ROLE,
            }))
        }
        DbCommand::Create { database, maintenance_secrets, owner, maintenance } => {
            create::run(
                config_dir,
                &database,
                &maintenance_secrets,
                &owner.owner_secrets,
                &maintenance,
            )
            .await
        }
        DbCommand::Preflight { database, output } => {
            evidence::preflight(config_dir, &database, &output).await
        }
        DbCommand::Inventory { database, output } => {
            evidence::inventory(config_dir, &database, &output).await
        }
        DbCommand::RetentionDryRun { database, output } => {
            evidence::retention_dry_run(config_dir, &database, &output).await
        }
        DbCommand::RetentionPurge {
            database,
            dry_run_receipt,
            approvals,
            execute,
            confirm,
            receipt,
            batch_size,
        } => {
            purge::run(purge::Options {
                config_dir,
                database: &database,
                dry_run_receipt: &dry_run_receipt,
                approvals: &approvals,
                execute,
                confirm: confirm.as_deref(),
                receipt: receipt.as_deref(),
                batch_size,
            })
            .await
        }
        DbCommand::AssetReports { database, output } => {
            assets::latest(config_dir, &database, &output).await
        }
        DbCommand::ReportSolvency { database, output } => {
            assets::report_solvency(config_dir, &database, &output).await
        }
        DbCommand::InspectConsistency { database, output } => {
            inspection::run(config_dir, &database, &output).await
        }
        DbCommand::Rebuild {
            database,
            maintenance_secrets,
            owner,
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
                owner_secrets: &owner.owner_secrets,
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

/// 读取迁移 owner 的独立敏感 TOML；错误信息固定脱敏，不回显连接串。
pub(crate) fn load_owner_secrets(
    path: &std::path::Path,
) -> anyhow::Result<auction_config::PostgresOwnerSecrets> {
    auction_config::load_postgres_owner_secrets(path).context("读取迁移 owner 凭据失败")
}

/// 连接应用数据库；调用方负责在结束前关闭连接池，优先使用 [`with_store`]。
pub(crate) async fn connect(
    config: &auction_config::AppConfig,
    purpose: auction_storage::DatabaseConnectionPurpose,
) -> anyhow::Result<DatabaseStore> {
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    connect_url(config, url, purpose).await
}

/// 连接 Worker 配置的数据库执行 `operation`；无论成败都先关闭连接池再返回结果。
pub(crate) async fn with_store<T>(
    config: &auction_config::AppConfig,
    purpose: auction_storage::DatabaseConnectionPurpose,
    operation: impl AsyncFnOnce(&DatabaseStore) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    closing(connect(config, purpose).await?, operation).await
}

/// 在已建立的连接池上执行 `operation`，任何返回路径都关闭连接池；用于迁移 owner 等显式连接。
pub(crate) async fn closing<T>(
    store: DatabaseStore,
    operation: impl AsyncFnOnce(&DatabaseStore) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let result = operation(&store).await;
    store.postgres_pool().close().await;
    result
}

/// 加载 Worker 配置并确认 `--database` 与连接目标的库名一致；连接之前调用。
pub(crate) fn confirmed_worker(
    config_dir: &ConfigDir,
    database: &str,
) -> anyhow::Result<auction_config::AppConfig> {
    let config = config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    DatabaseTarget::parse(url)?.require_database(database)?;
    Ok(config)
}

/// 用显式连接串（服务账号或迁移 owner）连接；用途档位仍来自同一配置。
pub(crate) async fn connect_url(
    config: &auction_config::AppConfig,
    url: &str,
    purpose: auction_storage::DatabaseConnectionPurpose,
) -> anyhow::Result<DatabaseStore> {
    DatabaseStore::connect(url, profile::resolve(config, purpose))
        .await
        .context("连接 PostgreSQL 失败")
}

/// 证明 owner 凭据指向同一数据库但账号不同：owner 需要 DDL，服务账号被迁移 081 收窄为无 DDL。
pub(crate) fn require_owner_target(
    config: &auction_config::AppConfig,
    owner: &auction_config::PostgresOwnerSecrets,
) -> anyhow::Result<DatabaseTarget> {
    let application_url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let application = DatabaseTarget::parse(application_url)?;
    let owner_target = DatabaseTarget::parse(owner.database_url())?;
    application.require_same_endpoint(&owner_target)?;
    if owner_target.database != application.database {
        anyhow::bail!("迁移 owner 凭据必须指向 Worker 配置中的同一数据库");
    }
    if owner_target.username == application.username {
        anyhow::bail!(
            "迁移 owner 账号不能与服务运行账号相同；owner 需要 DDL，服务账号必须是 {}",
            auction_config::RUNTIME_DATABASE_ROLE
        );
    }
    if let Some(declared) = config.database_roles.owner.as_deref()
        && declared != owner_target.username
    {
        anyhow::bail!(
            "common.toml 的 [database_roles] owner（{declared}）与 --owner-secrets 账号不一致；两者必须指向同一迁移 owner"
        );
    }
    Ok(owner_target)
}

/// 编译制品内嵌保留政策中加密邮件诊断文件的两条期限，返回（一般诊断保留天数，写入未知证据
/// 保全天数）。只读演练与实际清理共用同一来源，缺少任一规则时失败关闭。
fn mail_diagnostic_windows() -> anyhow::Result<(i64, i64)> {
    let ordinary_days =
        auction_storage::retention_policy_rule_duration_days("mail_diagnostic_ordinary_age")
            .context("政策 v2 缺少一般诊断保留期限")?;
    let hold_days =
        auction_storage::retention_policy_rule_duration_days("mail_diagnostic_write_unknown_hold")
            .context("政策 v2 缺少写入未知证据保全期限")?;
    Ok((ordinary_days, hold_days))
}

/// 把政策天数换算为时长；负数失败关闭，极大值按秒数饱和。
fn days_to_duration(days: i64) -> anyhow::Result<std::time::Duration> {
    let days = u64::try_from(days).context("政策期限超出范围")?;
    Ok(std::time::Duration::from_secs(days.saturating_mul(86_400)))
}

/// 只读读取目标身份与迁移状态；history 不是完整成功前缀时失败。
pub(crate) async fn preflight(
    store: &DatabaseStore,
    database: &str,
) -> anyhow::Result<PostgresRecoveryPreflight> {
    store.collect_recovery_preflight(database).await.context("读取数据库身份与迁移历史失败")
}

#[derive(Debug, Serialize)]
pub(crate) struct MigrationReport {
    pub(crate) database: String,
    pub(crate) before_version: i64,
    pub(crate) after_version: i64,
    pub(crate) expected_version: i64,
    pub(crate) applied: i64,
}

/// 在 SQLx 迁移锁内以迁移 owner 执行全部待执行迁移；空库会建立完整 schema，
/// 未知非空库被拒绝。服务运行账号没有 DDL 权限，因此不能用于本路径。迁移后版本必须等于
/// 制品内嵌的期望版本：数据库来自更新的制品时 `migrate` 只是空操作，不能报告成功。
pub(crate) async fn apply_migrations(
    config: &auction_config::AppConfig,
    owner: &auction_config::PostgresOwnerSecrets,
) -> anyhow::Result<MigrationReport> {
    let target = require_owner_target(config, owner)?;
    let store = connect_url(
        config,
        owner.database_url(),
        auction_storage::DatabaseConnectionPurpose::CliMigrate,
    )
    .await?;
    closing(store, async |store| {
        let before = latest_version(&preflight(store, &target.database).await?);
        progress(format_args!("迁移 {}：当前版本 {before}", target.database));
        store.migrate().await.context("执行数据库迁移失败")?;
        let after = latest_version(&preflight(store, &target.database).await?);
        let expected_version = auction_storage::postgres_expected_migration_version();
        if after != expected_version {
            anyhow::bail!(
                "迁移后数据库版本 {after} 与制品期望版本 {expected_version} 不一致\
                 （数据库可能由更新的制品迁移过，或迁移未全部应用）；保持停机核对后再启动服务"
            );
        }
        Ok(MigrationReport {
            database: target.database.clone(),
            before_version: before,
            after_version: after,
            expected_version,
            applied: after - before,
        })
    })
    .await
}

fn latest_version(preflight: &PostgresRecoveryPreflight) -> i64 {
    preflight.migration.as_ref().map_or(0, |migration| migration.latest_version)
}

async fn status(config_dir: &ConfigDir) -> anyhow::Result<()> {
    let config = config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let target = DatabaseTarget::parse(url)?;
    let preflight =
        with_store(&config, auction_storage::DatabaseConnectionPurpose::CliStatus, async |store| {
            preflight(store, &target.database).await
        })
        .await?;
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
