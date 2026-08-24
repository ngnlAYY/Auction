//! PostgreSQL 单事务恢复命令。
//!
//! 本模块只恢复到已确认的空数据库，不拥有 `DROP DATABASE`、`CREATE DATABASE` 或会话终止。
//! 外部部署编排必须先停止全部角色和客户端；恢复工具负责 archive 身份、空库预检、单事务执行、
//! 恢复后 schema 证明和不可覆盖 receipt。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_storage::{
    DatabaseBackend, DatabaseStore, PostgresRecoveryPreflight, postgres_expected_migration_version,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    PostgresBackupManifest, PostgresCommandEnvironment, create_private_temp_path,
    manifest_archive_path, read_backup_manifest, require_absent_artifact, run_external_command,
    sha256_file, sha256_text, validate_backup_manifest, validate_private_evidence_path,
    verify_archive, verify_private_regular_file,
};

const POSTGRES_RESTORE_RECEIPT_FORMAT: &str = "auction-postgres-restore.v1";

#[derive(Debug, Serialize)]
struct PostgresRestoreReceipt {
    format: &'static str,
    restored_at: DateTime<Utc>,
    database_name: String,
    database_name_sha256: String,
    archive_file_name: String,
    archive_sha256: String,
    manifest_sha256: String,
    schema_version: i64,
    single_transaction: bool,
    ownership_restored: bool,
    privileges_restored: bool,
}

/// 执行 restore 或只读 dry-run。
pub(super) async fn execute(
    manifest_path: &Path,
    receipt_path: Option<&Path>,
    confirmed_database_name: &str,
    dry_run: bool,
) -> anyhow::Result<()> {
    match (dry_run, receipt_path) {
        (true, Some(_)) => {
            anyhow::bail!("postgres-restore --dry-run 不写 receipt，请移除 --receipt")
        }
        (false, None) => anyhow::bail!("实际 postgres-restore 必须提供 --receipt"),
        _ => {}
    }

    let verified = verify_backup_pair(manifest_path, confirmed_database_name)?;
    let expected_binary_version = postgres_expected_migration_version();
    if verified.manifest.schema_version > expected_binary_version {
        anyhow::bail!(
            "backup schema version {} is newer than this binary's PostgreSQL migration version {}",
            verified.manifest.schema_version,
            expected_binary_version,
        );
    }

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for PostgreSQL restore")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("postgres-restore 只支持 PostgreSQL 完整档");
    }
    let command_environment =
        PostgresCommandEnvironment::from_database_url(database_url, confirmed_database_name)?;
    let mut receipt = receipt_path.map(RestoreReceiptReservation::reserve).transpose()?;

    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for PostgreSQL restore preflight")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("postgres-restore 只支持 PostgreSQL 完整档"),
    };
    let preflight = postgres
        .collect_recovery_preflight(confirmed_database_name)
        .await
        .context("collect PostgreSQL restore preflight")?;
    require_empty_restore_target(&preflight)?;
    // 自身连接必须先关闭，否则 pg_restore 阶段会制造一个并非外部客户端的额外 session。
    postgres.close().await;

    if dry_run {
        println!(
            "postgres-restore dry-run passed archive={} manifest={} database={} schema_version={} active_sessions={} user_objects={}",
            verified.archive_path.display(),
            manifest_path.display(),
            confirmed_database_name,
            verified.manifest.schema_version,
            preflight.active_other_sessions,
            preflight.user_object_count,
        );
        return Ok(());
    }

    // 空库检查和实际 mutation 之间再次读取两个 artifact，拒绝 TOCTOU 替换或内容变化。
    let reverified = verify_backup_pair(manifest_path, confirmed_database_name)?;
    if reverified.manifest_sha256 != verified.manifest_sha256
        || reverified.archive_path != verified.archive_path
        || reverified.manifest.archive_sha256 != verified.manifest.archive_sha256
        || reverified.manifest.schema_version != verified.manifest.schema_version
    {
        anyhow::bail!("backup archive or manifest changed during PostgreSQL restore preflight");
    }

    run_pg_restore(&command_environment, confirmed_database_name, &reverified.archive_path)?;

    let restored_store = DatabaseStore::connect(database_url)
        .await
        .context("restore committed but reconnecting for schema verification failed")?;
    let restored_postgres = match restored_store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("restore committed but target backend no longer resolves to PostgreSQL")
        }
    };
    let restored = restored_postgres
        .collect_recovery_preflight(confirmed_database_name)
        .await
        .context("restore committed but collecting restored schema evidence failed")?;
    require_restored_schema(&restored, reverified.manifest.schema_version)?;
    restored_postgres.close().await;

    let receipt_payload = PostgresRestoreReceipt {
        format: POSTGRES_RESTORE_RECEIPT_FORMAT,
        restored_at: Utc::now(),
        database_name: confirmed_database_name.to_owned(),
        database_name_sha256: sha256_text(confirmed_database_name),
        archive_file_name: reverified.manifest.archive_file_name.clone(),
        archive_sha256: reverified.manifest.archive_sha256.clone(),
        manifest_sha256: reverified.manifest_sha256,
        schema_version: reverified.manifest.schema_version,
        single_transaction: true,
        ownership_restored: false,
        privileges_restored: false,
    };
    let receipt_bytes =
        serde_json::to_vec_pretty(&receipt_payload).context("encode PostgreSQL restore receipt")?;
    let receipt_reservation = receipt.take().ok_or_else(|| {
        anyhow::anyhow!("restore committed but the reserved receipt unexpectedly disappeared")
    })?;
    receipt_reservation.publish(&receipt_bytes)?;
    let receipt_path = receipt_path.ok_or_else(|| {
        anyhow::anyhow!("restore committed but the required receipt path unexpectedly disappeared")
    })?;

    println!(
        "postgres-restore completed database={} schema_version={} archive_sha256={} receipt={}",
        confirmed_database_name,
        receipt_payload.schema_version,
        receipt_payload.archive_sha256,
        receipt_path.display(),
    );
    Ok(())
}

pub(super) struct VerifiedBackup {
    pub(super) manifest: PostgresBackupManifest,
    pub(super) archive_path: PathBuf,
    pub(super) manifest_sha256: String,
}

pub(super) fn verify_backup_pair(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<VerifiedBackup> {
    validate_private_evidence_path(manifest_path)?;
    let manifest = read_backup_manifest(manifest_path)?;
    validate_backup_manifest(&manifest, confirmed_database_name)?;
    let archive_path = manifest_archive_path(manifest_path, &manifest)?;
    validate_private_evidence_path(&archive_path)?;
    verify_private_regular_file(&archive_path, "backup archive")?;
    let actual_archive_sha256 = sha256_file(&archive_path)?;
    if actual_archive_sha256 != manifest.archive_sha256 {
        anyhow::bail!("backup archive SHA-256 does not match its manifest");
    }
    verify_archive(&archive_path)?;
    Ok(VerifiedBackup { manifest, archive_path, manifest_sha256: sha256_file(manifest_path)? })
}

pub(super) fn require_empty_restore_target(
    preflight: &PostgresRecoveryPreflight,
) -> anyhow::Result<()> {
    if !preflight.restore_write_possible {
        anyhow::bail!("PostgreSQL restore target is read-only or a recovery replica");
    }
    if preflight.active_other_sessions != 0 {
        anyhow::bail!(
            "PostgreSQL restore target has {} other session(s); stop all roles and clients first",
            preflight.active_other_sessions,
        );
    }
    if preflight.user_object_count != 0
        || preflight.schema_identity_present
        || preflight.migration_history_present
        || !preflight.restore_preflight_passed
    {
        anyhow::bail!(
            "PostgreSQL restore target is not an empty database: user_objects={} schema_identity_present={} migration_history_present={}",
            preflight.user_object_count,
            preflight.schema_identity_present,
            preflight.migration_history_present,
        );
    }
    Ok(())
}

fn require_restored_schema(
    restored: &PostgresRecoveryPreflight,
    expected_schema_version: i64,
) -> anyhow::Result<()> {
    let identity = restored
        .schema_identity
        .as_ref()
        .filter(|identity| identity.application == "auction" && identity.baseline_version > 0)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "restore committed but Auction schema identity is missing or invalid; keep all roles stopped"
            )
        })?;
    let migration = restored
        .migration
        .as_ref()
        .filter(|migration| {
            migration.migration_count > 0
                && migration.all_success
                && migration.latest_version == expected_schema_version
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "restore committed but migration history does not match schema version {expected_schema_version}; keep all roles stopped"
            )
        })?;
    if restored.database_name.is_empty()
        || restored.in_recovery
        || restored.default_transaction_read_only
        || identity.application != "auction"
        || migration.latest_version != expected_schema_version
    {
        anyhow::bail!(
            "restore committed but target schema evidence is invalid; keep all roles stopped"
        );
    }
    Ok(())
}

fn run_pg_restore(
    environment: &PostgresCommandEnvironment,
    confirmed_database_name: &str,
    archive_path: &Path,
) -> anyhow::Result<()> {
    // `pg_restore --dbname` 会把包含 `=` 的裸数据库名解释为 libpq conninfo。显式构造只含
    // dbname 的安全 conninfo，host/user/TLS 继续来自已经清洗过的 PG* 环境，且命令行不含凭据。
    let database_conninfo =
        format!("dbname='{}'", confirmed_database_name.replace('\\', "\\\\").replace('\'', "\\'"));
    let mut command = ProcessCommand::new("pg_restore");
    command
        .args([
            "--no-password",
            "--exit-on-error",
            "--single-transaction",
            "--no-owner",
            "--no-privileges",
            "--dbname",
        ])
        .arg(database_conninfo)
        .arg(archive_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    environment.apply(&mut command);
    run_external_command(&mut command, "pg_restore")
        .context("PostgreSQL restore failed; single transaction should have rolled back")
}

struct RestoreReceiptReservation {
    target_path: PathBuf,
    temporary_path: PathBuf,
    published: bool,
}

impl RestoreReceiptReservation {
    fn reserve(target_path: &Path) -> anyhow::Result<Self> {
        validate_private_evidence_path(target_path)?;
        require_absent_artifact(target_path, "restore receipt")?;
        let parent = target_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("restore receipt parent is missing"))?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("read restore receipt parent: {}", parent.display()))?;
        if !metadata.file_type().is_dir() {
            anyhow::bail!("restore receipt parent is not a directory: {}", parent.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o022 != 0 {
                anyhow::bail!(
                    "restore receipt parent is group/other writable: {}",
                    parent.display()
                );
            }
        }
        let temporary_path =
            create_private_temp_path(target_path, "auction-postgres-restore-receipt")?;
        Ok(Self { target_path: target_path.to_owned(), temporary_path, published: false })
    }

    fn publish(mut self, contents: &[u8]) -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.temporary_path)
            .with_context(|| {
                format!("open restore receipt temporary file: {}", self.temporary_path.display())
            })?;
        file.write_all(contents).context("write PostgreSQL restore receipt")?;
        file.write_all(b"\n").context("terminate PostgreSQL restore receipt")?;
        file.sync_all().context("sync PostgreSQL restore receipt")?;
        drop(file);

        require_absent_artifact(&self.target_path, "restore receipt")?;
        fs::hard_link(&self.temporary_path, &self.target_path).with_context(|| {
            format!("publish PostgreSQL restore receipt: {}", self.target_path.display())
        })?;
        fs::remove_file(&self.temporary_path).with_context(|| {
            format!(
                "restore committed and receipt published, but temporary receipt cleanup failed: {}",
                self.temporary_path.display()
            )
        })?;
        self.published = true;
        Ok(())
    }
}

impl Drop for RestoreReceiptReservation {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}
