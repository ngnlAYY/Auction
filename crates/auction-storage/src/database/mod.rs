#[cfg(feature = "postgres")]
mod admin_intake;
#[cfg(feature = "postgres")]
mod admin_mail;
#[cfg(feature = "postgres")]
mod asset_reconciliation;
#[cfg(feature = "postgres")]
pub use asset_reconciliation::{
    CUSTODY_REPORT_ACTION, CUSTODY_REPORT_ENTITY, CUSTODY_REPORT_FORMAT, POSTAGE_RATE_CREDITS,
    POSTAGE_RATE_VERIFIED, SOLVENCY_REPORT_ACTION, SOLVENCY_REPORT_ENTITY, SOLVENCY_REPORT_FORMAT,
};
#[cfg(feature = "postgres")]
mod audit_health;
#[cfg(feature = "postgres")]
mod bootstrap;
#[cfg(feature = "postgres")]
mod initialization;
#[cfg(feature = "postgres")]
pub use initialization::{FirstDatabaseInitializationError, initialize_empty_postgres};
#[cfg(feature = "postgres")]
mod competition;
#[cfg(feature = "postgres")]
mod competition_audit;
#[cfg(feature = "postgres")]
mod consistency_inspection;
#[cfg(feature = "postgres")]
mod deploy_evidence;
#[cfg(feature = "postgres")]
pub use consistency_inspection::{
    ConsistencyCheckReport, ConsistencyInspectionError, ConsistencyInspectionReport,
    ConsistencyViolation,
};
#[cfg(feature = "postgres")]
pub use deploy_evidence::{
    DeployEvidenceError, InFlightMailSend, InFlightPmSend, MigrationCompatibilityEntry,
    PostgresInFlightSends, PostgresMigrationSuffix,
};
#[cfg(feature = "postgres")]
mod equipment_loss;
#[cfg(feature = "postgres")]
mod external_recovery;
#[cfg(feature = "test-db")]
pub use equipment_loss::delivery_snapshot_equality_sql;
#[cfg(feature = "postgres")]
pub use external_recovery::{
    EXTERNAL_RECOVERY_QUARANTINE_ACTION, EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE,
};
#[cfg(feature = "postgres")]
mod intake_target;
#[cfg(feature = "postgres")]
mod key_rotation;
#[cfg(feature = "postgres")]
pub use key_rotation::{
    KEY_ROTATION_PAGE_SIZE, KeyRotationError, RotatedKeyEnvelope, StoredKeyEnvelope,
    append_reencrypt_audit, replace_key_envelope, scan_key_envelopes, stored_key_ids,
};
#[cfg(feature = "postgres")]
mod least_privilege;
#[cfg(feature = "postgres")]
mod lifecycle_audit;
#[cfg(feature = "postgres")]
mod mail_delivery;
#[cfg(feature = "postgres")]
mod mail_intake;
#[cfg(feature = "postgres")]
mod mail_poll;
#[cfg(feature = "postgres")]
mod mail_scan_checkpoint;
#[cfg(feature = "postgres")]
mod mailbox_sweep;
#[cfg(feature = "postgres")]
pub use least_privilege::{apply_runtime_privileges, runtime_privileges_present};
#[cfg(feature = "postgres")]
mod migrations;
#[cfg(feature = "test-db")]
pub use migrations::postgres_migrate_to;
#[cfg(feature = "postgres")]
mod rebuild;
#[cfg(feature = "postgres")]
mod recovery_preflight;
#[cfg(feature = "postgres")]
mod sent_confirmation;
#[cfg(feature = "postgres")]
mod settlement;
#[cfg(feature = "postgres")]
pub mod settlement_funding;
#[cfg(feature = "postgres")]
mod settlement_policy;
#[cfg(feature = "postgres")]
mod settlement_report;
#[cfg(feature = "postgres")]
mod shutdown_window;
#[cfg(feature = "postgres")]
mod slow_proxy_due;
#[cfg(feature = "postgres")]
pub use bootstrap::{
    PostgresBootstrapResult, PostgresDatabaseBootstrap, PostgresDatabaseBootstrapError,
};
#[cfg(feature = "test-db")]
pub use competition::BidQueueMode;
#[cfg(feature = "postgres")]
pub use competition::PostgresCompetitionTransaction;
#[cfg(feature = "postgres")]
pub use rebuild::{
    POSTGRES_REBUILD_PREFLIGHT_FORMAT, PostgresDatabaseRebuildGuard, PostgresRebuildError,
    PostgresRebuildPreflight, PostgresRebuildResult,
};
#[cfg(feature = "postgres")]
pub use recovery_preflight::{
    PostgresRecoveryPreflight, RECOVERY_PREFLIGHT_FORMAT, RecoveryPreflightError,
};
#[cfg(feature = "postgres")]
pub use retention_inventory::{
    PostgresForeignKeyCoverage, PostgresForeignKeyInventory, PostgresForeignKeyInventoryReport,
    PostgresMigrationInventory, PostgresRealtimeCoverage, PostgresRealtimeInventory,
    PostgresRealtimeStreamInventory, PostgresRelationInventory, PostgresRetentionFamilyInventory,
    PostgresRetentionInventory, PostgresRetentionTableInventory, PostgresSchemaIdentity,
    RETENTION_INVENTORY_FORMAT, RetentionInventoryError, RetentionPolicy, RetentionPolicyFamily,
    RetentionPolicyRetention, RetentionPolicyRule, RetentionRestoreObjectives,
};
#[cfg(feature = "postgres")]
pub use shutdown_window::{
    PostgresShutdownWindow, ShutdownEndingLot, ShutdownFinalAuction, ShutdownWindowError,
};
#[cfg(feature = "postgres")]
mod commands;
#[cfg(feature = "postgres")]
mod compensation;
#[cfg(feature = "postgres")]
mod compensation_reversal;
#[cfg(feature = "postgres")]
mod credentials;
#[cfg(feature = "postgres")]
pub use commands::{
    DatabaseAdminAuctionCommands, DatabaseAdminIntakeCommands, DatabaseAdminLotCommands,
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAdminWorldCommands, DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands,
};
#[cfg(feature = "postgres")]
pub use credentials::{CredentialRuntimeOwnership, CredentialRuntimeOwnershipError};
#[cfg(feature = "postgres")]
pub use credentials::{
    MailEvidenceTransactionError, lock_mail_evidence_credentials, lock_mail_runtime_generation,
    validate_mail_evidence_time,
};
#[cfg(feature = "postgres")]
mod account;
#[cfg(feature = "postgres")]
mod admin_auctions;
#[cfg(feature = "postgres")]
mod admin_audit;
#[cfg(feature = "postgres")]
mod admin_lots;
#[cfg(feature = "postgres")]
mod admin_proxy_events;
#[cfg(feature = "postgres")]
mod admin_renames;
#[cfg(feature = "postgres")]
mod auctions;
#[cfg(feature = "postgres")]
mod bid_log;
#[cfg(feature = "postgres")]
mod blacklist;
#[cfg(feature = "postgres")]
mod delivery_admin;
#[cfg(feature = "postgres")]
mod delivery_audit;
#[cfg(feature = "postgres")]
mod delivery_disposition;
#[cfg(feature = "postgres")]
mod delivery_scheduler;
#[cfg(feature = "postgres")]
mod equipment_secrets;
#[cfg(feature = "postgres")]
mod forum;
#[cfg(feature = "postgres")]
mod fulfillment_scheduler;
#[cfg(feature = "postgres")]
mod health;
#[cfg(feature = "postgres")]
mod lot_watch;
#[cfg(feature = "postgres")]
mod lot_watch_pm;
#[cfg(feature = "postgres")]
mod lots;
#[cfg(feature = "postgres")]
pub use lots::DatabasePublicLotReader;
#[cfg(feature = "postgres")]
mod legacy_self_proof;
#[cfg(feature = "test-db")]
pub use legacy_self_proof::{trusted_delivery_predicate_sql, trusted_obligation_equality_sql};
#[cfg(feature = "postgres")]
mod operations_alert;
#[cfg(feature = "postgres")]
mod player_notifications;
#[cfg(feature = "postgres")]
mod price_history;
#[cfg(feature = "test-db")]
pub use price_history::benchmark_sql;
#[cfg(feature = "postgres")]
mod public_equipment_link;
#[cfg(feature = "postgres")]
mod realtime;
#[cfg(all(feature = "postgres", feature = "test-db"))]
pub use realtime::sequence::benchmark_sql as realtime_benchmark_sql;
#[cfg(feature = "postgres")]
mod realtime_admin;
#[cfg(feature = "postgres")]
mod realtime_consumer;
#[cfg(feature = "postgres")]
pub use realtime_consumer::{RealtimeConsumerOwnership, RealtimeConsumerOwnershipError};
#[cfg(feature = "postgres")]
mod recovery_commands;
#[cfg(feature = "postgres")]
mod recovery_health;
#[cfg(feature = "postgres")]
mod recovery_report;
#[cfg(feature = "postgres")]
mod recovery_scheduler;
#[cfg(feature = "postgres")]
mod retention_dry_run;
#[cfg(feature = "postgres")]
mod retention_inventory;
#[cfg(feature = "postgres")]
mod retention_purge;
#[cfg(feature = "postgres")]
pub use retention_dry_run::{
    RETENTION_DRY_RUN_FORMAT, RetentionDryRunError, RetentionDryRunReport, RetentionFamilyDryRun,
    RetentionRuleDryRun, RetentionTableDryRun,
};
#[cfg(feature = "postgres")]
pub use retention_purge::{
    RETENTION_PURGE_APPROVAL_FORMAT, RETENTION_PURGE_DEFAULT_BATCH_SIZE,
    RETENTION_PURGE_DRY_RUN_MAX_DURATION_SECONDS, RETENTION_PURGE_MAX_BATCH_SIZE,
    RETENTION_PURGE_MIN_BATCH_SIZE, RETENTION_PURGE_POLICY_VERSION, RETENTION_PURGE_RECEIPT_FORMAT,
    RETENTION_PURGE_RECEIPT_MAX_AGE_SECONDS, RetentionPurgeApproval, RetentionPurgeError,
    RetentionPurgeFamilyResult, RetentionPurgeFilesystemResult, RetentionPurgeOutcome,
    RetentionPurgeReport, RetentionPurgeRequest, RetentionPurgeTableApproval,
    RetentionPurgeTableResult, retention_purge_approval_token,
};
#[cfg(feature = "postgres")]
mod rules;
#[cfg(feature = "postgres")]
mod scheduler;
#[cfg(feature = "postgres")]
mod sessions;
#[cfg(feature = "postgres")]
mod settlement_catalog;
#[cfg(feature = "postgres")]
mod settlement_commands;
#[cfg(feature = "postgres")]
mod settlement_scheduler;
#[cfg(feature = "postgres")]
pub use settlement_commands::DatabaseSettlementCommands;
#[cfg(feature = "postgres")]
mod users;
#[cfg(feature = "postgres")]
mod worlds;

#[cfg(feature = "postgres")]
mod pool;
#[cfg(feature = "postgres")]
pub use pool::TrackedTransaction;
#[cfg(feature = "postgres")]
pub(crate) use pool::{TrackedConnection, TrackedPool};

#[cfg(feature = "postgres")]
mod profiles;
#[cfg(feature = "postgres")]
pub use profiles::{
    DatabaseConnectionProfile, DatabaseConnectionPurpose, DatabaseConnectionTimeouts,
};

mod backend;
#[cfg(feature = "postgres")]
mod postgres_store;
#[cfg(feature = "postgres")]
mod store;

pub use backend::{DatabaseBackend, StorageError};
#[cfg(feature = "postgres")]
pub(crate) use backend::{
    SERIALIZABLE_ATTEMPTS, lock_command_id, observe_retry_exhaustion,
    observed_serialization_conflict, retry_conflicts, serializable_retry_backoff,
    set_read_only_limits, sql_error_context, storage_error, storage_error_from,
};
#[cfg(feature = "postgres")]
pub(crate) use backend::{
    quote_identifier, valid_confirmed_database_name, valid_ddl_database_name,
};
#[cfg(feature = "postgres")]
pub use postgres_store::{PostgresStore, postgres_expected_migration_version};
/// 政策 v2 的规则期限（天）；运维回执用它记录文件族期限，不授权变更。
#[cfg(feature = "postgres")]
pub fn retention_policy_rule_duration_days(rule_id: &str) -> Option<i64> {
    retention_inventory::retention_policy_rule_duration_days(rule_id)
}

/// 编译制品内嵌政策 v2 的版本号与 SHA-256；批准与回执用它固定政策身份。
#[cfg(feature = "postgres")]
pub fn retention_policy_identity() -> Option<(i64, String)> {
    retention_inventory::retention_policy_identity()
}
#[cfg(feature = "postgres")]
pub use store::DatabaseStore;

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

#[cfg(feature = "postgres")]
mod runtime_switches;

#[cfg(feature = "postgres")]
mod sli;
