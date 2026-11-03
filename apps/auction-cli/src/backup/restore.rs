//! 单事务恢复：只恢复到 manifest 对应的同名空库（零其他连接、可写主节点、无用户对象），
//! 恢复后核对 schema 身份与迁移历史；不建库、不删库、不终止他人连接。

use std::path::Path;

use anyhow::Context;
use auction_storage::PostgresRecoveryPreflight;
use chrono::Utc;

use super::{libpq::Libpq, verify_pair};
use crate::app::{print_json, progress};
use crate::cli::MaintenanceArgs;
use crate::deployment::ConfigDir;
use crate::support::files;

const RECEIPT_FORMAT: &str = "auction-postgres-restore.v1";

pub(super) struct Options<'a> {
    pub(super) config_dir: &'a ConfigDir,
    pub(super) manifest: &'a Path,
    pub(super) database: &'a str,
    pub(super) dry_run: bool,
    pub(super) receipt: Option<&'a Path>,
    pub(super) maintenance: &'a MaintenanceArgs,
}

pub(super) async fn run(options: Options<'_>) -> anyhow::Result<()> {
    if let Some(receipt) = options.receipt {
        files::require_private_target(receipt)?;
    }
    let verified = verify_pair(options.manifest, options.database).await?;
    verified.require_supported_schema()?;
    let guard =
        crate::service::acquire_maintenance(options.config_dir, options.maintenance).await?;
    let config = options.config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    let environment = Libpq::from_url(
        url,
        options.database,
        config.database_profiles.mode,
        auction_storage::DatabaseConnectionPurpose::CliRestore,
    )?;

    let store =
        crate::database::connect(&config, auction_storage::DatabaseConnectionPurpose::CliRestore)
            .await?;
    let result = crate::database::preflight(&store, options.database).await;
    // 自身连接必须先关闭，否则 pg_restore 阶段会多出一个非外部客户端的会话。
    store.postgres_pool().close().await;
    require_empty_target(&result?)?;

    if options.dry_run {
        return print_json(&serde_json::json!({
            "outcome": "dry_run", "guard": guard.mode, "database": options.database,
            "archive": verified.archive, "schema_version": verified.manifest.schema_version,
        }));
    }
    let receipt = options.receipt.context("实际恢复必须提供 --receipt")?;
    // 空库检查与恢复之间重新校验恢复点，拒绝被替换或改写的 archive/manifest。
    let current = verify_pair(options.manifest, options.database).await?;
    if current.manifest_sha256 != verified.manifest_sha256 {
        anyhow::bail!("备份 manifest 在预检期间发生变化，已停止恢复");
    }
    progress(format_args!("pg_restore → {}", options.database));
    environment
        .restore(options.database, &current.archive)
        .await
        .context("恢复失败；单事务恢复应已整体回滚，请用 db preflight 复核目标仍为空库")?;

    let store =
        crate::database::connect(&config, auction_storage::DatabaseConnectionPurpose::CliRestore)
            .await
            .context("恢复已提交，但重新连接校验失败")?;
    let restored = crate::database::preflight(&store, options.database).await;
    store.postgres_pool().close().await;
    let restored = restored.context("恢复已提交，但读取恢复后的 schema 失败；保持停机")?;
    let migration = restored
        .migration
        .as_ref()
        .filter(|migration| {
            restored.schema_identity_present
                && migration.latest_version == current.manifest.schema_version
        })
        .context("恢复已提交，但 schema 身份或迁移历史与备份不一致；保持停机并人工核对")?;
    let payload = serde_json::json!({
        "format": RECEIPT_FORMAT,
        "restored_at": Utc::now(),
        "database": options.database,
        "archive_file_name": current.manifest.archive_file_name,
        "archive_sha256": current.manifest.archive_sha256,
        "manifest_sha256": current.manifest_sha256,
        "schema_version": migration.latest_version,
        "single_transaction": true,
        "ownership_restored": false,
        "privileges_restored": false,
    });
    files::write_private(receipt, &serde_json::to_vec_pretty(&payload)?)
        .context("恢复已完成，但回执写入失败")?;
    print_json(&serde_json::json!({
        "outcome": "restored", "guard": guard.mode, "database": options.database,
        "schema_version": migration.latest_version, "receipt": receipt,
        "next_step": if migration.current { "auction-cli service start" } else { "auction-cli db migrate" },
    }))
}

fn require_empty_target(preflight: &PostgresRecoveryPreflight) -> anyhow::Result<()> {
    if !preflight.restore_write_possible {
        anyhow::bail!("恢复目标是只读节点或处于 recovery");
    }
    if preflight.active_other_sessions != 0 {
        anyhow::bail!(
            "恢复目标仍有 {} 个其他连接；先停止全部角色与客户端",
            preflight.active_other_sessions
        );
    }
    if !preflight.restore_preflight_passed {
        anyhow::bail!(
            "恢复目标不是空库（用户对象 {}，schema 身份 {}，迁移历史 {}）；可用 db rebuild --mode empty-for-restore 准备空库",
            preflight.user_object_count,
            preflight.schema_identity_present,
            preflight.migration_history_present
        );
    }
    Ok(())
}
