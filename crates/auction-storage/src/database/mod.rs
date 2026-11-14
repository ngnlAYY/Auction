//! PostgreSQL 存储适配器的模块装配与公开导出。

mod accounts;
mod admin;
mod auction;
mod delivery;
mod mail;
mod maintenance;
mod operations;
mod public;
mod recovery;
#[cfg(feature = "test-db")]
pub use delivery::equipment_loss::delivery_snapshot_equality_sql;
pub use maintenance::deploy_evidence::{
    DeployEvidenceError, InFlightMailSend, InFlightPmSend, MigrationCompatibilityEntry,
    PostgresInFlightSends, PostgresMigrationSuffix,
};
pub use maintenance::external_recovery::{
    EXTERNAL_RECOVERY_BLOCK_ACTION, EXTERNAL_RECOVERY_PAYLOAD_QUARANTINE_FAILURE_CODE,
    EXTERNAL_RECOVERY_QUARANTINE_ACTION, EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE,
    EXTERNAL_RECOVERY_UNRESOLVED_FAILURE_CODE,
};
pub use maintenance::initialization::{
    FirstDatabaseInitializationError, initialize_empty_postgres,
};
pub use maintenance::key_rotation::{
    KEY_ROTATION_PAGE_SIZE, KeyRotationError, RotatedKeyEnvelope, StoredKeyEnvelope,
    append_reencrypt_audit, replace_admin_command_key_envelope, replace_key_envelope,
    scan_admin_command_key_envelopes, scan_key_envelopes, stored_key_ids,
};
pub use maintenance::least_privilege::{apply_runtime_privileges, runtime_privileges_present};
pub use operations::asset_reconciliation::{
    CUSTODY_REPORT_ACTION, CUSTODY_REPORT_ENTITY, CUSTODY_REPORT_FORMAT, POSTAGE_RATE_CREDITS,
    POSTAGE_RATE_VERIFIED, SOLVENCY_REPORT_ACTION, SOLVENCY_REPORT_ENTITY, SOLVENCY_REPORT_FORMAT,
};
pub use operations::audit_health::install_audit_shadow_recorder;
pub use operations::consistency_inspection::{
    ConsistencyCheckReport, ConsistencyInspectionError, ConsistencyInspectionReport,
    ConsistencyViolation,
};
mod migrations;
#[cfg(feature = "test-db")]
pub use migrations::postgres_migrate_to;
mod settlement;
#[cfg(feature = "test-db")]
pub use auction::competition::BidQueueMode;
pub use auction::competition::PostgresCompetitionTransaction;
pub use maintenance::bootstrap::{
    PostgresBootstrapResult, PostgresDatabaseBootstrap, PostgresDatabaseBootstrapError,
};
pub use maintenance::rebuild::{
    POSTGRES_REBUILD_PREFLIGHT_FORMAT, PostgresDatabaseRebuildGuard, PostgresRebuildError,
    PostgresRebuildPreflight, PostgresRebuildResult,
};
pub use maintenance::recovery_preflight::{
    PostgresRecoveryPreflight, RECOVERY_PREFLIGHT_FORMAT, RecoveryPreflightError,
};
pub use maintenance::retention::inventory::{
    PostgresForeignKeyCoverage, PostgresForeignKeyInventory, PostgresForeignKeyInventoryReport,
    PostgresMigrationInventory, PostgresRealtimeCoverage, PostgresRealtimeInventory,
    PostgresRealtimeStreamInventory, PostgresRelationInventory, PostgresRetentionFamilyInventory,
    PostgresRetentionInventory, PostgresRetentionTableInventory, PostgresSchemaIdentity,
    RETENTION_INVENTORY_FORMAT, RetentionInventoryError, RetentionPolicy, RetentionPolicyFamily,
    RetentionPolicyRetention, RetentionPolicyRule, RetentionRestoreObjectives,
};
pub use maintenance::shutdown_window::{
    PostgresShutdownWindow, ShutdownEndingLot, ShutdownFinalAuction, ShutdownWindowError,
};
mod commands;
pub use accounts::credentials::{CredentialRuntimeOwnership, CredentialRuntimeOwnershipError};
pub use accounts::credentials::{
    MailEvidenceTransactionError, lock_mail_evidence_credentials, lock_mail_runtime_generation,
    validate_mail_evidence_time,
};
pub use commands::{
    DatabaseAdminAuctionCommands, DatabaseAdminIntakeCommands, DatabaseAdminLotCommands,
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAdminWorldCommands, DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands,
};
mod forum;
mod lot_watch;
#[cfg(feature = "test-db")]
pub use delivery::legacy_self_proof::{
    trusted_delivery_predicate_sql, trusted_obligation_equality_sql,
};
pub use public::lots::DatabasePublicLotReader;
#[cfg(feature = "test-db")]
pub use public::price_history::benchmark_sql;
mod realtime;
pub use maintenance::retention::dry_run::{
    RETENTION_DRY_RUN_FORMAT, RetentionDryRunError, RetentionDryRunReport, RetentionFamilyDryRun,
    RetentionRuleDryRun, RetentionTableDryRun,
};
pub use maintenance::retention::purge::{
    RETENTION_PURGE_APPROVAL_FORMAT, RETENTION_PURGE_DEFAULT_BATCH_SIZE,
    RETENTION_PURGE_DRY_RUN_MAX_DURATION_SECONDS, RETENTION_PURGE_MAX_BATCH_SIZE,
    RETENTION_PURGE_MIN_BATCH_SIZE, RETENTION_PURGE_POLICY_VERSION, RETENTION_PURGE_RECEIPT_FORMAT,
    RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS, RetentionPurgeApproval, RetentionPurgeError,
    RetentionPurgeFamilyResult, RetentionPurgeFilesystemResult, RetentionPurgeOutcome,
    RetentionPurgeReport, RetentionPurgeRequest, RetentionPurgeTableApproval,
    RetentionPurgeTableResult, retention_purge_approval_token,
};
pub use realtime::consumer::{RealtimeConsumerOwnership, RealtimeConsumerOwnershipError};
#[cfg(feature = "test-db")]
pub use realtime::sequence::benchmark_sql as realtime_benchmark_sql;
pub use settlement::commands::DatabaseSettlementCommands;

mod pool;
pub use pool::TrackedTransaction;
pub(crate) use pool::{TrackedConnection, TrackedPool};

mod profiles;
pub use profiles::{
    DatabaseConnectionProfile, DatabaseConnectionPurpose, DatabaseConnectionTimeouts,
};

mod backend;
mod store;

pub(crate) use backend::repository_error;
pub use backend::{DatabaseBackend, StorageError};
pub(crate) use backend::{
    SERIALIZABLE_ATTEMPTS, lock_command_id, observe_retry_exhaustion,
    observed_serialization_conflict, retry_conflicts, serializable_retry_backoff,
    serialization_failure, set_read_only_limits, sql_error_context, sql_error_kind,
    sql_error_sqlstate, storage_error, storage_error_from, storage_failure,
};
pub(crate) use backend::{
    quote_identifier, valid_confirmed_database_name, valid_ddl_database_name,
};
pub use store::{DatabaseStore, postgres_expected_migration_version};
/// 政策 v2 的规则期限（天）；运维回执用它记录文件族期限，不授权变更。
pub fn retention_policy_rule_duration_days(rule_id: &str) -> Option<i64> {
    maintenance::retention::inventory::retention_policy_rule_duration_days(rule_id)
}

/// 编译制品内嵌政策 v2 的版本号与 SHA-256；批准与回执用它固定政策身份。
pub fn retention_policy_identity() -> Option<(i64, String)> {
    maintenance::retention::inventory::retention_policy_identity()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_database_urls() {
        assert_eq!(DatabaseBackend::from_url("postgres://db").unwrap(), DatabaseBackend::Postgres);
        assert_eq!(
            DatabaseBackend::from_url("postgresql://db").unwrap(),
            DatabaseBackend::Postgres
        );
    }

    #[test]
    fn rejects_unknown_database_urls_without_echoing_credentials() {
        let error = DatabaseBackend::from_url("mysql://user:secret@example/db").unwrap_err();

        assert!(matches!(error, StorageError::UnsupportedScheme(ref scheme) if scheme == "mysql"));
        assert!(!error.to_string().contains("secret"));
    }
}
