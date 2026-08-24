//! PostgreSQL v28 接管命令的证据模型与格式常量。

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(super) const LEGACY_V28_BACKUP_FORMAT: &str = "auction-postgres-legacy-v28-backup.v1";
pub(super) const LEGACY_V28_ADOPTION_RECEIPT_FORMAT: &str =
    "auction-postgres-legacy-v28-adoption.v1";
pub(super) const MAX_LEGACY_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyV28BackupManifest {
    pub(super) format: String,
    pub(super) archive_file_name: String,
    pub(super) database_name_sha256: String,
    pub(super) archive_sha256: String,
    pub(super) legacy_migration_version: i64,
    pub(super) created_at: DateTime<Utc>,
    pub(super) preflight_sha256: String,
}

#[derive(Debug, Serialize)]
pub(super) struct LegacyV28AdoptionReceipt {
    pub(super) format: &'static str,
    pub(super) adopted_at: DateTime<Utc>,
    pub(super) database_name_sha256: String,
    pub(super) archive_file_name: String,
    pub(super) archive_sha256: String,
    pub(super) manifest_sha256: String,
    pub(super) preflight_sha256: String,
    pub(super) legacy_migration_version: i64,
    pub(super) baselined_sqlx_versions: Vec<i64>,
    pub(super) final_sqlx_version: i64,
    pub(super) audit_id: Uuid,
}

#[derive(Debug, Serialize)]
pub(super) struct LegacyV28PreflightFingerprint<'a> {
    pub(super) format: &'a str,
    pub(super) database_name_sha256: String,
    pub(super) postgres_major: i32,
    pub(super) in_recovery: bool,
    pub(super) default_transaction_read_only: bool,
    pub(super) active_other_sessions: i64,
    pub(super) sqlx_history_present: bool,
    pub(super) legacy_history_present: bool,
    pub(super) legacy_history_row_count: i64,
    pub(super) legacy_version: Option<i64>,
    pub(super) legacy_dirty: Option<bool>,
    pub(super) schema_identity_valid: bool,
    pub(super) expected_table_count: usize,
    pub(super) actual_table_count: usize,
    pub(super) missing_tables: &'a [String],
    pub(super) unexpected_tables: &'a [String],
    pub(super) source_shape_issue_count: i64,
    pub(super) bridge_target_object_count: i64,
    pub(super) policy_issue_count: i64,
    pub(super) lot_projection_issue_count: i64,
    pub(super) bid_event_issue_count: i64,
    pub(super) adoption_ready: bool,
}

pub(super) struct VerifiedLegacyBackup {
    pub(super) manifest: LegacyV28BackupManifest,
    pub(super) archive_path: PathBuf,
    pub(super) manifest_sha256: String,
}
