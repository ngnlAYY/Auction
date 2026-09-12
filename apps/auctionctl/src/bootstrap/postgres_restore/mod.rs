//! PostgreSQL 单事务恢复命令。
//!
//! 本模块只恢复到已确认的空数据库，不拥有 DROP/CREATE DATABASE 或会话终止。
//! 恢复编排、备份/模式证明、外部 pg_restore 进程和不可覆盖 receipt 分别归档到子模块。

use std::path::{Path, PathBuf};

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_storage::{
    DatabaseBackend, DatabaseStore, PostgresRecoveryPreflight, postgres_expected_migration_version,
};
use chrono::Utc;

use super::{
    PostgresBackupManifest, PostgresCommandEnvironment, PrivateReceiptReservation,
    RestoreReceiptSpec, manifest_archive_path, read_backup_manifest, run_external_command,
    sha256_file, sha256_text, validate_backup_manifest, validate_private_evidence_path,
    verify_archive, verify_private_regular_file,
};

mod process;
mod receipt;
mod verification;

use process::run_pg_restore;
use receipt::{PostgresRestoreReceipt, RestoreReceiptReservation};

const POSTGRES_RESTORE_RECEIPT_FORMAT: &str = "auction-postgres-restore.v1";

#[derive(Debug)]
pub(super) struct VerifiedBackup {
    pub(super) manifest: PostgresBackupManifest,
    pub(super) archive_path: PathBuf,
    pub(super) manifest_sha256: String,
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

    super::sync_private_artifact(&reverified.archive_path)?;
    super::sync_private_artifact(manifest_path)?;
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
    verification::require_restored_schema(&restored, reverified.manifest.schema_version)?;
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

pub(super) fn verify_backup_pair(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<VerifiedBackup> {
    verification::verify_backup_pair(manifest_path, confirmed_database_name)
}

pub(super) fn require_empty_restore_target(
    preflight: &PostgresRecoveryPreflight,
) -> anyhow::Result<()> {
    verification::require_empty_restore_target(preflight)
}
