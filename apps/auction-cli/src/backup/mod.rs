//! PostgreSQL 备份、离线校验与单事务恢复；外部 pg_dump/pg_restore 由部署环境提供。

use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use anyhow::Context;

use crate::app::print_json;
use crate::cli::BackupCommand;
use crate::config_dir::ConfigDir;

mod create;
mod drill_evidence;
mod libpq;
mod manifest;
mod restore;
mod scheduled;
pub(in crate::backup) mod scope;
pub(crate) mod wal;

pub(crate) use create::create_backup;
pub(crate) use drill_evidence::latest_restore_drill;
pub(crate) use manifest::BackupManifest;
pub(crate) use scheduled::{TerminalOutcome, latest_terminal_run};
pub(crate) use wal::staged_wal_facts;

/// 异机对象键：空 prefix 表示 bucket 根下的固定布局；非空时只加一层分隔，避免生成前导
/// 斜杠。定时备份副本与 WAL/基础备份上传共用。
pub(crate) fn object_key(prefix: &str, tail: &str) -> String {
    if prefix.is_empty() { tail.to_owned() } else { format!("{prefix}/{tail}") }
}

/// 通过 manifest、摘要与 archive 目录三重校验的恢复点。
pub(crate) struct VerifiedBackup {
    pub(crate) manifest: BackupManifest,
    pub(crate) archive: PathBuf,
    /// Owner-only unlinked copy; pg_restore consumes exactly the bytes whose digest was checked.
    pub(crate) archive_file: File,
    pub(crate) manifest_sha256: String,
    /// 备份范围 sidecar 存在时一并校验；旧恢复点没有 sidecar 时为 None。
    pub(crate) scope: Option<scope::VerifiedScope>,
}

impl VerifiedBackup {
    /// 恢复与为恢复准备空库共用版本门禁，不能删库后才发现当前工具无法装载恢复点。
    pub(crate) fn require_supported_schema(&self) -> anyhow::Result<()> {
        let expected = auction_storage::postgres_expected_migration_version();
        if self.manifest.schema_version > expected {
            anyhow::bail!(
                "备份 schema 版本 {} 高于当前二进制支持的 {expected}；请使用对应版本的 auction-cli",
                self.manifest.schema_version
            );
        }
        Ok(())
    }
}

pub(crate) async fn run(config_dir: &ConfigDir, command: BackupCommand) -> anyhow::Result<()> {
    match command {
        BackupCommand::Scheduled { database, output_dir, state_dir } => {
            scheduled::run(config_dir, &database, &output_dir, &state_dir).await
        }
        BackupCommand::Create { database, output } => {
            create::run(config_dir, &database, &output).await
        }
        BackupCommand::Verify { manifest, database } => {
            let verified = verify_pair(&manifest, &database).await?;
            print_json(&serde_json::json!({
                "verified": true,
                "archive": verified.archive,
                "archive_sha256": verified.manifest.archive_sha256,
                "manifest_sha256": verified.manifest_sha256,
                "schema_version": verified.manifest.schema_version,
                "created_at": verified.manifest.created_at,
                "scope": verified.scope.as_ref().map(scope::VerifiedScope::projection),
            }))
        }
        BackupCommand::ScopeExtract { manifest, database, identity, output_dir, receipt } => {
            scope::extract::run(scope::extract::Options {
                manifest: &manifest,
                database: &database,
                identity: &identity,
                output_dir: &output_dir,
                receipt: receipt.as_deref(),
            })
            .await
        }
        BackupCommand::Restore { manifest, database, dry_run, receipt, owner, maintenance } => {
            restore::run(restore::Options {
                config_dir,
                owner_secrets: &owner.owner_secrets,
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
    let archive_file = verified_archive_snapshot(&archive, &manifest.archive_sha256)?;
    libpq::list_archive(&archive_file).await?;
    // 范围 sidecar 只与 archive 配对；存在时必须通过摘要校验，缺失则按旧恢复点处理。
    let scope = scope::verify(&archive, &manifest.archive_file_name).await?;
    Ok(VerifiedBackup { manifest, archive, archive_file, manifest_sha256, scope })
}

fn verified_archive_snapshot(path: &Path, expected_sha256: &str) -> anyhow::Result<File> {
    let mut source = crate::support::files::open_regular(path)?;
    let directory = std::env::temp_dir();
    let (snapshot_path, mut snapshot) = (0..8)
        .find_map(|_| {
            let candidate =
                directory.join(format!("auction-restore-{}.partial", uuid::Uuid::new_v4()));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&candidate)
            {
                Ok(file) => Some(Ok((candidate, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .context("无法分配唯一的恢复 archive 快照")??;
    let result = (|| -> anyhow::Result<File> {
        std::io::copy(&mut source, &mut snapshot).context("复制已打开的恢复 archive 失败")?;
        snapshot.sync_all().context("同步恢复 archive 快照失败")?;
        snapshot.seek(SeekFrom::Start(0)).context("定位恢复 archive 快照失败")?;
        let (actual_sha256, _) = crate::support::digest::sha256_reader(&mut snapshot)?;
        if actual_sha256 != expected_sha256 {
            anyhow::bail!("archive 的 SHA-256 与 manifest 不一致：{}", path.display());
        }
        snapshot.seek(SeekFrom::Start(0)).context("重置恢复 archive 快照位置失败")?;
        Ok(snapshot)
    })();
    let _ = fs::remove_file(snapshot_path);
    result
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
