//! Auction-BF PostgreSQL v28 的专用备份、校验和接管命令。
//!
//! 旧版 golang-migrate 历史不能伪装成普通 Rust SQLx 备份。本模块使用独立 manifest，
//! 在任何数据库写入前要求经过离线校验的 archive、稳定源指纹、零其他连接、逐字确认和
//! 已预留的不可覆盖 receipt。实际 schema/data bridge 仍由 `auction-storage` 独占。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_storage::{
    DatabaseBackend, DatabaseStore, LEGACY_V28_MIGRATION_VERSION, PostgresLegacyV28Preflight,
    PostgresRecoveryPreflight,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    PostgresCommandEnvironment, backup_manifest_path, create_private_temp_path, is_sha256_hex,
    publish_private_artifact, remove_owned_temp, require_absent_artifact, require_dump_extension,
    run_pg_dump, sha256_file, sha256_text, validate_private_evidence_path, verify_archive,
    verify_private_regular_file, write_private_evidence,
};

const LEGACY_V28_BACKUP_FORMAT: &str = "auction-postgres-legacy-v28-backup.v1";
const LEGACY_V28_ADOPTION_RECEIPT_FORMAT: &str = "auction-postgres-legacy-v28-adoption.v1";
const MAX_LEGACY_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyV28BackupManifest {
    format: String,
    archive_file_name: String,
    database_name_sha256: String,
    archive_sha256: String,
    legacy_migration_version: i64,
    created_at: DateTime<Utc>,
    preflight_sha256: String,
}

#[derive(Debug, Serialize)]
struct LegacyV28AdoptionReceipt {
    format: &'static str,
    adopted_at: DateTime<Utc>,
    database_name_sha256: String,
    archive_file_name: String,
    archive_sha256: String,
    manifest_sha256: String,
    preflight_sha256: String,
    legacy_migration_version: i64,
    baselined_sqlx_versions: Vec<i64>,
    final_sqlx_version: i64,
    audit_id: Uuid,
}

#[derive(Debug, Serialize)]
struct LegacyV28PreflightFingerprint<'a> {
    format: &'a str,
    database_name_sha256: String,
    postgres_major: i32,
    in_recovery: bool,
    default_transaction_read_only: bool,
    active_other_sessions: i64,
    sqlx_history_present: bool,
    legacy_history_present: bool,
    legacy_history_row_count: i64,
    legacy_version: Option<i64>,
    legacy_dirty: Option<bool>,
    schema_identity_valid: bool,
    expected_table_count: usize,
    actual_table_count: usize,
    missing_tables: &'a [String],
    unexpected_tables: &'a [String],
    source_shape_issue_count: i64,
    bridge_target_object_count: i64,
    policy_issue_count: i64,
    lot_projection_issue_count: i64,
    bid_event_issue_count: i64,
    adoption_ready: bool,
}

struct VerifiedLegacyBackup {
    manifest: LegacyV28BackupManifest,
    archive_path: PathBuf,
    manifest_sha256: String,
}

/// 为严格通过预检的 canonical Auction-BF v28 创建专用备份。
pub(super) async fn backup(output: &Path, confirmed_database_name: &str) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    require_dump_extension(output)?;
    let manifest_path = backup_manifest_path(output)?;
    validate_private_evidence_path(&manifest_path)?;
    require_absent_artifact(output, "legacy v28 backup archive")?;
    require_absent_artifact(&manifest_path, "legacy v28 backup manifest")?;

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for legacy v28 backup")?;
    require_postgres_backend(database_url)?;
    let command_environment =
        PostgresCommandEnvironment::from_database_url(database_url, confirmed_database_name)?;
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for legacy v28 backup")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("postgres-legacy-v28-backup 只支持 PostgreSQL 完整档")
        }
    };

    let before = postgres
        .collect_legacy_v28_preflight(confirmed_database_name)
        .await
        .context("inspect legacy v28 source before pg_dump")?;
    require_adoption_ready(&before)?;
    let source_preflight_sha256 = preflight_sha256(&before)?;
    let temporary_archive = create_private_temp_path(output, "auction-postgres-legacy-v28-backup")?;
    let backup_result = async {
        run_pg_dump(&command_environment, &temporary_archive)?;
        verify_archive(&temporary_archive)?;

        let after = postgres
            .collect_legacy_v28_preflight(confirmed_database_name)
            .await
            .context("inspect legacy v28 source after pg_dump")?;
        require_adoption_ready(&after)?;
        let after_preflight_sha256 = preflight_sha256(&after)?;
        if after_preflight_sha256 != source_preflight_sha256 {
            anyhow::bail!(
                "legacy v28 source shape changed during pg_dump; discard this backup attempt"
            );
        }

        publish_private_artifact(&temporary_archive, output)?;
        let archive_file_name = output
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("legacy backup archive filename is not valid UTF-8"))?
            .to_owned();
        let manifest = LegacyV28BackupManifest {
            format: LEGACY_V28_BACKUP_FORMAT.to_owned(),
            archive_file_name,
            database_name_sha256: sha256_text(confirmed_database_name),
            archive_sha256: sha256_file(output)?,
            legacy_migration_version: LEGACY_V28_MIGRATION_VERSION,
            created_at: Utc::now(),
            preflight_sha256: source_preflight_sha256,
        };
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).context("encode legacy v28 backup manifest")?;
        write_private_evidence(&manifest_path, &manifest_bytes)?;
        Ok::<LegacyV28BackupManifest, anyhow::Error>(manifest)
    }
    .await;
    postgres.close().await;
    if backup_result.is_err() {
        remove_owned_temp(&temporary_archive);
    }
    let manifest = backup_result?;

    println!(
        "postgres-legacy-v28-backup written archive={} manifest={} legacy_version={} archive_sha256={} preflight_sha256={}",
        output.display(),
        manifest_path.display(),
        manifest.legacy_migration_version,
        manifest.archive_sha256,
        manifest.preflight_sha256,
    );
    Ok(())
}

/// 离线校验专用旧版 archive/manifest 对。
pub(super) fn verify_backup_cli(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    let verified = verify_backup(manifest_path, confirmed_database_name)?;
    println!(
        "postgres-legacy-v28-backup-verify passed archive={} manifest={} legacy_version={} archive_sha256={} preflight_sha256={}",
        verified.archive_path.display(),
        manifest_path.display(),
        verified.manifest.legacy_migration_version,
        verified.manifest.archive_sha256,
        verified.manifest.preflight_sha256,
    );
    Ok(())
}

/// 默认只读演练；只有所有外部证据齐备并逐字确认后才执行接管。
pub(super) async fn adopt(
    confirmed_database_name: &str,
    backup_manifest: Option<&Path>,
    execute: bool,
    confirm_adoption: Option<&str>,
    receipt_path: Option<&Path>,
) -> anyhow::Result<()> {
    if !execute && (confirm_adoption.is_some() || receipt_path.is_some()) {
        anyhow::bail!("postgres-adopt-legacy-v28 dry-run 不接受 --confirm-adoption 或 --receipt");
    }
    let verified =
        backup_manifest.map(|path| verify_backup(path, confirmed_database_name)).transpose()?;
    if execute {
        let expected_confirmation = format!("ADOPT LEGACY V28 {confirmed_database_name}");
        if confirm_adoption != Some(expected_confirmation.as_str()) {
            anyhow::bail!("actual adoption requires --confirm-adoption '{expected_confirmation}'");
        }
        if verified.is_none() {
            anyhow::bail!("actual adoption requires --backup-manifest");
        }
        if receipt_path.is_none() {
            anyhow::bail!("actual adoption requires --receipt");
        }
    }

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for legacy v28 adoption")?;
    require_postgres_backend(database_url)?;
    // 同时解析外部工具连接环境，确保配置中的数据库路径也精确匹配人工确认；本命令不会
    // 调用外部工具，但沿用 backup 的严格 libpq URL 边界可以避免两个命令解释目标不同。
    let _command_environment =
        PostgresCommandEnvironment::from_database_url(database_url, confirmed_database_name)?;
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for legacy v28 adoption")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("postgres-adopt-legacy-v28 只支持 PostgreSQL 完整档")
        }
    };
    let preflight = postgres
        .collect_legacy_v28_preflight(confirmed_database_name)
        .await
        .context("collect legacy v28 adoption preflight")?;
    require_adoption_ready(&preflight)?;
    let current_preflight_sha256 = preflight_sha256(&preflight)?;

    if !execute {
        if let Some(verified) = &verified
            && verified.manifest.preflight_sha256 != current_preflight_sha256
        {
            anyhow::bail!(
                "live legacy v28 source shape does not match the supplied backup manifest"
            );
        }
        postgres.close().await;
        println!(
            "postgres-adopt-legacy-v28 dry-run passed database={} legacy_version={} tables={} source_shape_issues={} policy_issues={} lot_projection_issues={} bid_event_issues={} backup_verified={}",
            confirmed_database_name,
            preflight.legacy_version.unwrap_or_default(),
            preflight.actual_table_count,
            preflight.source_shape_issue_count,
            preflight.policy_issue_count,
            preflight.lot_projection_issue_count,
            preflight.bid_event_issue_count,
            verified.is_some(),
        );
        return Ok(());
    }

    let receipt_path = receipt_path
        .ok_or_else(|| anyhow::anyhow!("required adoption receipt path disappeared"))?;
    let mut receipt = Some(AdoptionReceiptReservation::reserve(receipt_path)?);
    let verified = verified
        .ok_or_else(|| anyhow::anyhow!("required legacy v28 backup evidence disappeared"))?;
    let manifest_path = backup_manifest
        .ok_or_else(|| anyhow::anyhow!("required backup manifest path disappeared"))?;

    // 数据库 mutation 前重新读取两个 artifact，拒绝预检期间发生的替换或内容变化。
    let reverified = verify_backup(manifest_path, confirmed_database_name)?;
    require_same_backup(&verified, &reverified)?;
    if reverified.manifest.preflight_sha256 != current_preflight_sha256 {
        anyhow::bail!("live legacy v28 source shape does not match the verified backup manifest");
    }

    let adoption = postgres
        .adopt_legacy_v28(confirmed_database_name)
        .await
        .context("adopt canonical Auction-BF PostgreSQL v28")?;
    let postflight = postgres
        .collect_recovery_preflight(confirmed_database_name)
        .await
        .context("legacy v28 adoption committed but postflight collection failed")?;
    require_adopted_schema(&postflight, adoption.final_sqlx_version)?;
    postgres.close().await;

    let receipt_payload = LegacyV28AdoptionReceipt {
        format: LEGACY_V28_ADOPTION_RECEIPT_FORMAT,
        adopted_at: adoption.adopted_at,
        database_name_sha256: sha256_text(confirmed_database_name),
        archive_file_name: reverified.manifest.archive_file_name,
        archive_sha256: reverified.manifest.archive_sha256,
        manifest_sha256: reverified.manifest_sha256,
        preflight_sha256: reverified.manifest.preflight_sha256,
        legacy_migration_version: adoption.legacy_version,
        baselined_sqlx_versions: adoption.baselined_sqlx_versions,
        final_sqlx_version: adoption.final_sqlx_version,
        audit_id: adoption.audit_id,
    };
    let receipt_bytes = serde_json::to_vec_pretty(&receipt_payload)
        .context("encode legacy v28 adoption receipt")?;
    receipt
        .take()
        .ok_or_else(|| anyhow::anyhow!("reserved adoption receipt disappeared"))?
        .publish(&receipt_bytes)?;

    println!(
        "postgres-adopt-legacy-v28 completed database={} legacy_version={} final_sqlx_version={} audit_id={} receipt={}",
        confirmed_database_name,
        receipt_payload.legacy_migration_version,
        receipt_payload.final_sqlx_version,
        receipt_payload.audit_id,
        receipt_path.display(),
    );
    Ok(())
}

fn require_postgres_backend(database_url: &str) -> anyhow::Result<()> {
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("legacy v28 operations only support the PostgreSQL full profile");
    }
    Ok(())
}

fn require_adoption_ready(preflight: &PostgresLegacyV28Preflight) -> anyhow::Result<()> {
    if !preflight.adoption_ready {
        anyhow::bail!(
            "legacy v28 preflight rejected: database={} server_version_num={} other_sessions={} legacy_version={:?} legacy_dirty={:?} missing_tables={} unexpected_tables={} source_shape_issues={} bridge_targets={} policy_issues={} lot_projection_issues={} bid_event_issues={}",
            preflight.database_name,
            preflight.server_version_num,
            preflight.active_other_sessions,
            preflight.legacy_version,
            preflight.legacy_dirty,
            preflight.missing_tables.len(),
            preflight.unexpected_tables.len(),
            preflight.source_shape_issue_count,
            preflight.bridge_target_object_count,
            preflight.policy_issue_count,
            preflight.lot_projection_issue_count,
            preflight.bid_event_issue_count,
        );
    }
    Ok(())
}

fn preflight_sha256(preflight: &PostgresLegacyV28Preflight) -> anyhow::Result<String> {
    let fingerprint = LegacyV28PreflightFingerprint {
        format: preflight.format,
        database_name_sha256: sha256_text(&preflight.database_name),
        postgres_major: preflight.server_version_num / 10_000,
        in_recovery: preflight.in_recovery,
        default_transaction_read_only: preflight.default_transaction_read_only,
        active_other_sessions: preflight.active_other_sessions,
        sqlx_history_present: preflight.sqlx_history_present,
        legacy_history_present: preflight.legacy_history_present,
        legacy_history_row_count: preflight.legacy_history_row_count,
        legacy_version: preflight.legacy_version,
        legacy_dirty: preflight.legacy_dirty,
        schema_identity_valid: preflight.schema_identity_valid,
        expected_table_count: preflight.expected_table_count,
        actual_table_count: preflight.actual_table_count,
        missing_tables: &preflight.missing_tables,
        unexpected_tables: &preflight.unexpected_tables,
        source_shape_issue_count: preflight.source_shape_issue_count,
        bridge_target_object_count: preflight.bridge_target_object_count,
        policy_issue_count: preflight.policy_issue_count,
        lot_projection_issue_count: preflight.lot_projection_issue_count,
        bid_event_issue_count: preflight.bid_event_issue_count,
        adoption_ready: preflight.adoption_ready,
    };
    let encoded =
        serde_json::to_string(&fingerprint).context("encode legacy v28 preflight fingerprint")?;
    Ok(sha256_text(&encoded))
}

fn verify_backup(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<VerifiedLegacyBackup> {
    validate_private_evidence_path(manifest_path)?;
    verify_private_regular_file(manifest_path, "legacy v28 backup manifest")?;
    if fs::metadata(manifest_path)?.len() > MAX_LEGACY_MANIFEST_BYTES {
        anyhow::bail!("legacy v28 backup manifest exceeds the bounded evidence size");
    }
    let manifest_bytes = fs::read(manifest_path)
        .with_context(|| format!("read legacy v28 backup manifest: {}", manifest_path.display()))?;
    let manifest: LegacyV28BackupManifest =
        serde_json::from_slice(&manifest_bytes).context("decode legacy v28 backup manifest")?;
    validate_manifest(&manifest, confirmed_database_name)?;

    let manifest_file_name = manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("legacy backup manifest filename is not valid UTF-8"))?;
    if manifest_file_name != format!("{}.manifest", manifest.archive_file_name) {
        anyhow::bail!("legacy v28 backup manifest filename is not paired with its archive");
    }
    let archive_path = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("legacy backup manifest parent is missing"))?
        .join(&manifest.archive_file_name);
    validate_private_evidence_path(&archive_path)?;
    verify_private_regular_file(&archive_path, "legacy v28 backup archive")?;
    let archive_sha256 = sha256_file(&archive_path)?;
    if archive_sha256 != manifest.archive_sha256 {
        anyhow::bail!("legacy v28 backup archive SHA-256 does not match its manifest");
    }
    verify_archive(&archive_path)?;

    Ok(VerifiedLegacyBackup {
        manifest,
        archive_path,
        manifest_sha256: sha256_file(manifest_path)?,
    })
}

fn validate_manifest(
    manifest: &LegacyV28BackupManifest,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    if manifest.format != LEGACY_V28_BACKUP_FORMAT
        || manifest.legacy_migration_version != LEGACY_V28_MIGRATION_VERSION
        || manifest.archive_file_name.is_empty()
        || manifest.archive_file_name != manifest.archive_file_name.trim()
        || manifest.archive_file_name.contains('/')
        || manifest.archive_file_name.contains('\\')
        || manifest.archive_file_name.chars().any(char::is_control)
        || !manifest.archive_file_name.ends_with(".dump")
        || !is_sha256_hex(&manifest.database_name_sha256)
        || !is_sha256_hex(&manifest.archive_sha256)
        || !is_sha256_hex(&manifest.preflight_sha256)
    {
        anyhow::bail!("legacy v28 backup manifest identity or path fields are invalid");
    }
    if manifest.database_name_sha256 != sha256_text(confirmed_database_name) {
        anyhow::bail!("confirmed database name does not match legacy v28 backup manifest");
    }
    Ok(())
}

fn require_same_backup(
    before: &VerifiedLegacyBackup,
    after: &VerifiedLegacyBackup,
) -> anyhow::Result<()> {
    if before.manifest_sha256 != after.manifest_sha256
        || before.archive_path != after.archive_path
        || before.manifest.archive_sha256 != after.manifest.archive_sha256
        || before.manifest.preflight_sha256 != after.manifest.preflight_sha256
        || before.manifest.legacy_migration_version != after.manifest.legacy_migration_version
    {
        anyhow::bail!("legacy v28 backup archive or manifest changed during adoption preflight");
    }
    Ok(())
}

fn require_adopted_schema(
    postflight: &PostgresRecoveryPreflight,
    expected_version: i64,
) -> anyhow::Result<()> {
    let identity = postflight
        .schema_identity
        .as_ref()
        .filter(|identity| identity.application == "auction" && identity.baseline_version > 0)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "legacy v28 adoption committed but Auction schema identity is invalid; keep all roles stopped"
            )
        })?;
    let migration = postflight
        .migration
        .as_ref()
        .filter(|migration| {
            migration.all_success
                && migration.current
                && migration.latest_version == expected_version
                && migration.expected_version == expected_version
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "legacy v28 adoption committed but SQLx migration history is not current at version {expected_version}; keep all roles stopped"
            )
        })?;
    if postflight.in_recovery
        || postflight.default_transaction_read_only
        || !postflight.schema_identity_present
        || !postflight.migration_history_present
        || identity.application != "auction"
        || migration.latest_version != expected_version
    {
        anyhow::bail!(
            "legacy v28 adoption committed but postflight schema evidence is invalid; keep all roles stopped"
        );
    }
    Ok(())
}

struct AdoptionReceiptReservation {
    target_path: PathBuf,
    temporary_path: PathBuf,
    published: bool,
}

impl AdoptionReceiptReservation {
    fn reserve(target_path: &Path) -> anyhow::Result<Self> {
        validate_private_evidence_path(target_path)?;
        require_absent_artifact(target_path, "legacy v28 adoption receipt")?;
        let parent = target_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("adoption receipt parent is missing"))?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("read adoption receipt parent: {}", parent.display()))?;
        if !metadata.file_type().is_dir() {
            anyhow::bail!("adoption receipt parent is not a directory: {}", parent.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o022 != 0 {
                anyhow::bail!(
                    "adoption receipt parent is group/other writable: {}",
                    parent.display()
                );
            }
        }
        let temporary_path =
            create_private_temp_path(target_path, "auction-postgres-legacy-v28-receipt")?;
        Ok(Self { target_path: target_path.to_owned(), temporary_path, published: false })
    }

    fn publish(mut self, contents: &[u8]) -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.temporary_path)
            .with_context(|| {
                format!("open adoption receipt temporary file: {}", self.temporary_path.display())
            })?;
        file.write_all(contents).context("write legacy v28 adoption receipt")?;
        file.write_all(b"\n").context("terminate legacy v28 adoption receipt")?;
        file.sync_all().context("sync legacy v28 adoption receipt")?;
        drop(file);

        require_absent_artifact(&self.target_path, "legacy v28 adoption receipt")?;
        fs::hard_link(&self.temporary_path, &self.target_path).with_context(|| {
            format!("publish legacy v28 adoption receipt: {}", self.target_path.display())
        })?;
        fs::remove_file(&self.temporary_path).with_context(|| {
            format!(
                "adoption committed and receipt published, but temporary receipt cleanup failed: {}",
                self.temporary_path.display()
            )
        })?;
        self.published = true;
        Ok(())
    }
}

impl Drop for AdoptionReceiptReservation {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}
