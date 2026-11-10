mod admin_intake;
mod admin_mail;
mod asset_reconciliation;
pub use asset_reconciliation::{
    CUSTODY_REPORT_ACTION, CUSTODY_REPORT_ENTITY, CUSTODY_REPORT_FORMAT, POSTAGE_RATE_CREDITS,
    POSTAGE_RATE_VERIFIED, SOLVENCY_REPORT_ACTION, SOLVENCY_REPORT_ENTITY, SOLVENCY_REPORT_FORMAT,
};
mod audit_health;
pub use audit_health::install_audit_shadow_recorder;
mod bootstrap;
mod initialization;
pub use initialization::{FirstDatabaseInitializationError, initialize_empty_postgres};
mod competition;
mod competition_audit;
mod consistency_inspection;
mod deploy_evidence;
pub use consistency_inspection::{
    ConsistencyCheckReport, ConsistencyInspectionError, ConsistencyInspectionReport,
    ConsistencyViolation,
};
pub use deploy_evidence::{
    DeployEvidenceError, InFlightMailSend, InFlightPmSend, MigrationCompatibilityEntry,
    PostgresInFlightSends, PostgresMigrationSuffix,
};
mod equipment_loss;
mod external_recovery;
#[cfg(feature = "test-db")]
pub use equipment_loss::delivery_snapshot_equality_sql;
pub use external_recovery::{
    EXTERNAL_RECOVERY_BLOCK_ACTION, EXTERNAL_RECOVERY_PAYLOAD_QUARANTINE_FAILURE_CODE,
    EXTERNAL_RECOVERY_QUARANTINE_ACTION, EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE,
    EXTERNAL_RECOVERY_UNRESOLVED_FAILURE_CODE,
};
mod intake_target;
mod key_rotation;
pub use key_rotation::{
    KEY_ROTATION_PAGE_SIZE, KeyRotationError, RotatedKeyEnvelope, StoredKeyEnvelope,
    append_reencrypt_audit, replace_admin_command_key_envelope, replace_key_envelope,
    scan_admin_command_key_envelopes, scan_key_envelopes, stored_key_ids,
};
mod least_privilege;
mod lifecycle_audit;
mod mail_delivery;
mod mail_intake;
mod mail_poll;
mod mail_scan_checkpoint;
mod mailbox_sweep;
pub use least_privilege::{apply_runtime_privileges, runtime_privileges_present};
mod migrations;
#[cfg(feature = "test-db")]
pub use migrations::postgres_migrate_to;
mod rebuild;
mod recovery_preflight;
mod sent_confirmation;
mod settlement;
pub mod settlement_funding;
mod settlement_policy;
mod settlement_report;
mod shutdown_window;
mod slow_proxy_due;
pub use bootstrap::{
    PostgresBootstrapResult, PostgresDatabaseBootstrap, PostgresDatabaseBootstrapError,
};
#[cfg(feature = "test-db")]
pub use competition::BidQueueMode;
pub use competition::PostgresCompetitionTransaction;
pub use rebuild::{
    POSTGRES_REBUILD_PREFLIGHT_FORMAT, PostgresDatabaseRebuildGuard, PostgresRebuildError,
    PostgresRebuildPreflight, PostgresRebuildResult,
};
pub use recovery_preflight::{
    PostgresRecoveryPreflight, RECOVERY_PREFLIGHT_FORMAT, RecoveryPreflightError,
};
pub use retention_inventory::{
    PostgresForeignKeyCoverage, PostgresForeignKeyInventory, PostgresForeignKeyInventoryReport,
    PostgresMigrationInventory, PostgresRealtimeCoverage, PostgresRealtimeInventory,
    PostgresRealtimeStreamInventory, PostgresRelationInventory, PostgresRetentionFamilyInventory,
    PostgresRetentionInventory, PostgresRetentionTableInventory, PostgresSchemaIdentity,
    RETENTION_INVENTORY_FORMAT, RetentionInventoryError, RetentionPolicy, RetentionPolicyFamily,
    RetentionPolicyRetention, RetentionPolicyRule, RetentionRestoreObjectives,
};
pub use shutdown_window::{
    PostgresShutdownWindow, ShutdownEndingLot, ShutdownFinalAuction, ShutdownWindowError,
};
mod commands;
mod compensation;
mod compensation_reversal;
mod credentials;
pub use commands::{
    DatabaseAdminAuctionCommands, DatabaseAdminIntakeCommands, DatabaseAdminLotCommands,
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAdminWorldCommands, DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands,
};
pub use credentials::{CredentialRuntimeOwnership, CredentialRuntimeOwnershipError};
pub use credentials::{
    MailEvidenceTransactionError, lock_mail_evidence_credentials, lock_mail_runtime_generation,
    validate_mail_evidence_time,
};
mod account;
mod admin_auctions;
mod admin_audit;
mod admin_lots;
mod admin_proxy_events;
mod admin_renames;
mod auctions;
mod bid_log;
mod blacklist;
mod delivery_admin;
mod delivery_audit;
mod delivery_disposition;
mod delivery_scheduler;
mod equipment_secrets;
mod forum;
mod fulfillment_scheduler;
mod health;
mod lot_watch;
mod lot_watch_pm;
mod lots;
pub use lots::DatabasePublicLotReader;
mod legacy_self_proof;
#[cfg(feature = "test-db")]
pub use legacy_self_proof::{trusted_delivery_predicate_sql, trusted_obligation_equality_sql};
mod operations_alert;
mod player_notifications;
mod price_history;
#[cfg(feature = "test-db")]
pub use price_history::benchmark_sql;
mod public_equipment_link;
mod realtime;
#[cfg(feature = "test-db")]
pub use realtime::sequence::benchmark_sql as realtime_benchmark_sql;
mod realtime_admin;
mod realtime_consumer;
pub use realtime_consumer::{RealtimeConsumerOwnership, RealtimeConsumerOwnershipError};
mod recovery_commands;
mod recovery_health;
mod recovery_report;
mod recovery_scheduler;
mod retention_dry_run;
mod retention_inventory;
mod retention_purge;
pub use retention_dry_run::{
    RETENTION_DRY_RUN_FORMAT, RetentionDryRunError, RetentionDryRunReport, RetentionFamilyDryRun,
    RetentionRuleDryRun, RetentionTableDryRun,
};
pub use retention_purge::{
    RETENTION_PURGE_APPROVAL_FORMAT, RETENTION_PURGE_DEFAULT_BATCH_SIZE,
    RETENTION_PURGE_DRY_RUN_MAX_DURATION_SECONDS, RETENTION_PURGE_MAX_BATCH_SIZE,
    RETENTION_PURGE_MIN_BATCH_SIZE, RETENTION_PURGE_POLICY_VERSION, RETENTION_PURGE_RECEIPT_FORMAT,
    RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS, RetentionPurgeApproval, RetentionPurgeError,
    RetentionPurgeFamilyResult, RetentionPurgeFilesystemResult, RetentionPurgeOutcome,
    RetentionPurgeReport, RetentionPurgeRequest, RetentionPurgeTableApproval,
    RetentionPurgeTableResult, retention_purge_approval_token,
};
mod rules;
mod scheduler;
mod sessions;
mod settlement_catalog;
mod settlement_commands;
mod settlement_scheduler;
pub use settlement_commands::DatabaseSettlementCommands;
mod users;
mod worlds;

mod pool;
pub use pool::TrackedTransaction;
pub(crate) use pool::{TrackedConnection, TrackedPool};

mod profiles;
pub use profiles::{
    DatabaseConnectionProfile, DatabaseConnectionPurpose, DatabaseConnectionTimeouts,
};

mod backend;
mod store;

pub use backend::{DatabaseBackend, StorageError};
pub(crate) use backend::{
    SERIALIZABLE_ATTEMPTS, lock_command_id, observe_retry_exhaustion,
    observed_serialization_conflict, retry_conflicts, serializable_retry_backoff,
    serialization_failure, set_read_only_limits, sql_error_context, storage_error,
    storage_error_from, storage_failure,
};
pub(crate) use backend::{
    quote_identifier, valid_confirmed_database_name, valid_ddl_database_name,
};
pub(crate) use backend::{repository_error, repository_failure};
pub use store::{DatabaseStore, postgres_expected_migration_version};
/// 政策 v2 的规则期限（天）；运维回执用它记录文件族期限，不授权变更。
pub fn retention_policy_rule_duration_days(rule_id: &str) -> Option<i64> {
    retention_inventory::retention_policy_rule_duration_days(rule_id)
}

/// 编译制品内嵌政策 v2 的版本号与 SHA-256；批准与回执用它固定政策身份。
pub fn retention_policy_identity() -> Option<(i64, String)> {
    retention_inventory::retention_policy_identity()
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

mod runtime_switches;

mod sli;
