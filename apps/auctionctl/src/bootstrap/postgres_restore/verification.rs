use super::*;

pub(super) fn verify_backup_pair(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<VerifiedBackup> {
    let (manifest, manifest_bytes) = read_backup_manifest(manifest_path)?;
    validate_backup_manifest(&manifest, confirmed_database_name)?;
    let archive_path = manifest_archive_path(manifest_path, &manifest)?;
    let actual_archive_sha256 = sha256_file(&archive_path)?;
    if actual_archive_sha256 != manifest.archive_sha256 {
        anyhow::bail!("backup archive SHA-256 does not match its manifest");
    }
    verify_archive(&archive_path)?;
    Ok(VerifiedBackup { manifest, archive_path, manifest_sha256: sha256_bytes(&manifest_bytes) })
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

pub(super) fn require_restored_schema(
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
