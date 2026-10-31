//! PostgreSQL 备份、离线校验与单事务恢复；外部 pg_dump/pg_restore 由部署环境提供。

use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::Utc;

use crate::app::{print_json, progress};
use crate::cli::BackupCommand;
use crate::database::DatabaseTarget;
use crate::deployment::ConfigDir;
use crate::support::files;

mod libpq;
mod manifest;
mod restore;

pub(crate) use manifest::BackupManifest;
use manifest::{FORMAT, manifest_file_name, sha256_bytes, sha256_file};

/// 通过 manifest、摘要与 archive 目录三重校验的恢复点。
pub(crate) struct VerifiedBackup {
    pub(crate) manifest: BackupManifest,
    pub(crate) archive: PathBuf,
    pub(crate) manifest_sha256: String,
}

pub(crate) async fn run(config_dir: &ConfigDir, command: BackupCommand) -> anyhow::Result<()> {
    match command {
        BackupCommand::Create { database, output } => create(config_dir, &database, &output).await,
        BackupCommand::Verify { manifest, database } => {
            let verified = verify_pair(&manifest, &database).await?;
            print_json(&serde_json::json!({
                "verified": true,
                "archive": verified.archive,
                "archive_sha256": verified.manifest.archive_sha256,
                "manifest_sha256": verified.manifest_sha256,
                "schema_version": verified.manifest.schema_version,
                "created_at": verified.manifest.created_at,
            }))
        }
        BackupCommand::Restore { manifest, database, dry_run, receipt, maintenance } => {
            restore::run(restore::Options {
                config_dir,
                manifest: &manifest,
                database: &database,
                dry_run,
                receipt: receipt.as_deref(),
                maintenance: &maintenance,
            })
            .await
        }
    }
}

/// 校验 manifest 字段与库名、重算 archive SHA-256，并用 `pg_restore --list` 解析 archive 目录。
pub(crate) async fn verify_pair(
    manifest_path: &Path,
    database: &str,
) -> anyhow::Result<VerifiedBackup> {
    if !manifest_path.is_absolute() {
        anyhow::bail!("--manifest 必须是绝对路径");
    }
    let (manifest, manifest_sha256) = BackupManifest::read(manifest_path)?;
    let archive = manifest.validate(manifest_path, database)?;
    if sha256_file(&archive)? != manifest.archive_sha256 {
        anyhow::bail!("archive 的 SHA-256 与 manifest 不一致：{}", archive.display());
    }
    libpq::list_archive(&archive).await?;
    Ok(VerifiedBackup { manifest, archive, manifest_sha256 })
}

/// 备份前后各读取一次迁移版本，拒绝在备份期间发生 schema 变化的 archive。
async fn create(config_dir: &ConfigDir, database: &str, output: &Path) -> anyhow::Result<()> {
    let archive_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            name.ends_with(".dump")
                && !name.starts_with('.')
                && name.trim() == *name
                && !name.contains('\\')
                && !name.chars().any(char::is_control)
        })
        .context("--output 必须是不含控制字符、反斜杠或首尾空白的 .dump 文件名")?;
    let manifest_path = output.with_file_name(manifest_file_name(archive_name));
    files::require_private_target(output)?;
    files::require_private_target(&manifest_path)?;

    let config = config_dir.load_worker()?;
    let url = config.database_url().context("Worker 配置缺少数据库连接")?;
    DatabaseTarget::parse(url)?.require_database(database)?;
    let environment = libpq::Libpq::from_url(url, database)?;
    let store = crate::database::connect(url).await?;
    let result: anyhow::Result<BackupManifest> = async {
        let before = schema_version(&store, database).await?;
        let temporary =
            output.with_file_name(format!(".{archive_name}.{}.partial", uuid::Uuid::new_v4()));
        progress(format_args!("pg_dump {database}（schema {before}）"));
        let dumped = async {
            environment.dump(&temporary).await?;
            libpq::list_archive(&temporary).await?;
            let after = schema_version(&store, database).await?;
            if after != before {
                anyhow::bail!("备份期间 schema 版本从 {before} 变为 {after}，archive 已丢弃");
            }
            files::open_regular(&temporary)?.sync_all().context("同步 archive 失败")?;
            // 硬链接发布不会覆盖同名文件；成功后删除临时名。
            std::fs::hard_link(&temporary, output)
                .with_context(|| format!("发布 archive 失败：{}", output.display()))?;
            Ok(())
        }
        .await;
        let _ = std::fs::remove_file(&temporary);
        dumped?;
        files::sync_parent(output)?;
        let manifest = BackupManifest {
            format: FORMAT.to_owned(),
            archive_file_name: archive_name.to_owned(),
            database_name_sha256: sha256_bytes(database.as_bytes()),
            archive_sha256: sha256_file(output)?,
            schema_version: before,
            created_at: Utc::now(),
        };
        files::write_private(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
        Ok(manifest)
    }
    .await;
    store.postgres_pool().close().await;
    let manifest = result?;
    print_json(&serde_json::json!({
        "archive": output,
        "manifest": manifest_path,
        "archive_sha256": manifest.archive_sha256,
        "schema_version": manifest.schema_version,
    }))
}

/// 备份源必须带有完整成功的迁移历史；返回其最高版本。
async fn schema_version(
    store: &auction_storage::DatabaseStore,
    database: &str,
) -> anyhow::Result<i64> {
    let preflight = crate::database::preflight(store, database).await?;
    preflight
        .migration
        .as_ref()
        .filter(|migration| migration.latest_version > 0 && preflight.schema_identity_present)
        .map(|migration| migration.latest_version)
        .context("备份源缺少 Auction schema 身份或迁移历史")
}
