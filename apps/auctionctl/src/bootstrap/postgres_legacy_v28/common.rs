//! PostgreSQL v28 接管在多个阶段共用的后端、预检和证据校验。

use anyhow::Context;
use auction_storage::{DatabaseBackend, PostgresLegacyV28Preflight, PostgresRecoveryPreflight};

use super::super::sha256_text;
use super::model::{LegacyV28PreflightFingerprint, VerifiedLegacyBackup};

pub(super) fn require_postgres_backend(database_url: &str) -> anyhow::Result<()> {
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("legacy v28 operations only support the PostgreSQL full profile");
    }
    Ok(())
}

pub(super) fn require_adoption_ready(preflight: &PostgresLegacyV28Preflight) -> anyhow::Result<()> {
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

pub(super) fn preflight_sha256(preflight: &PostgresLegacyV28Preflight) -> anyhow::Result<String> {
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

pub(super) fn require_same_backup(
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

pub(super) fn require_adopted_schema(
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
