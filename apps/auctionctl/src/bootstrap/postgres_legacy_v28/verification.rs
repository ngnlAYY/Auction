//! PostgreSQL v28 archive、manifest 和文件路径的离线验证。

use std::{fs, path::Path};

use anyhow::Context;
use auction_storage::LEGACY_V28_MIGRATION_VERSION;

use super::super::{
    is_sha256_hex, sha256_file, sha256_text, validate_private_evidence_path, verify_archive,
    verify_private_regular_file,
};
use super::model::{
    LEGACY_V28_BACKUP_FORMAT, LegacyV28BackupManifest, MAX_LEGACY_MANIFEST_BYTES,
    VerifiedLegacyBackup,
};

/// 离线校验专用旧版 archive/manifest 对。
pub(in crate::bootstrap) fn verify_backup_cli(
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

pub(super) fn verify_backup(
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
