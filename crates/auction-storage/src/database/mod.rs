#[cfg(feature = "postgres")]
use std::sync::Arc;

#[cfg(feature = "postgres")]
use auction_application::{ApplicationError, ProcessReadinessPort, RepositoryError};
#[cfg(feature = "postgres")]
use auction_observability::{HeartbeatHealthReader, HeartbeatReadinessFuture, PoolMetricsSource};
#[cfg(feature = "postgres")]
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::{PgPool, Postgres, postgres::PgPoolOptions};
#[cfg(feature = "postgres")]
mod admin_intake;
#[cfg(feature = "postgres")]
mod admin_mail;
#[cfg(feature = "postgres")]
pub use admin_mail::{MailDisplayBackfillInput, MailDisplayBackfillResult};
#[cfg(feature = "postgres")]
mod audit_health;
#[cfg(feature = "postgres")]
mod bootstrap;
#[cfg(feature = "postgres")]
mod initialization;
#[cfg(feature = "postgres")]
pub use initialization::{FirstDatabaseInitializationError, initialize_empty_postgres};
#[cfg(feature = "postgres")]
#[path = "competition/postgres.rs"]
mod competition;
#[cfg(feature = "postgres")]
mod competition_audit;
#[cfg(feature = "postgres")]
mod legacy_v28;
#[cfg(feature = "postgres")]
mod lifecycle_audit;
#[cfg(feature = "postgres")]
mod mail_delivery;
#[cfg(feature = "postgres")]
mod mail_intake;
#[cfg(feature = "postgres")]
mod mail_poll;
#[cfg(feature = "postgres")]
mod migrations;
#[cfg(feature = "postgres")]
mod rebuild;
#[cfg(feature = "postgres")]
mod recovery_preflight;
#[cfg(feature = "postgres")]
mod settlement;
#[cfg(feature = "postgres")]
mod settlement_policy;
#[cfg(feature = "postgres")]
mod settlement_report;
#[cfg(feature = "postgres")]
mod slow_proxy_due;
#[cfg(feature = "postgres")]
pub use bootstrap::{
    PostgresBootstrapResult, PostgresDatabaseBootstrap, PostgresDatabaseBootstrapError,
};
#[cfg(feature = "postgres")]
pub use competition::PostgresCompetitionTransaction;
#[cfg(feature = "postgres")]
pub use legacy_v28::{
    LEGACY_V28_MIGRATION_VERSION, POSTGRES_LEGACY_V28_PREFLIGHT_FORMAT, PostgresLegacyV28Adoption,
    PostgresLegacyV28Error, PostgresLegacyV28Preflight,
};
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
};
#[cfg(feature = "postgres")]
mod commands;
#[cfg(feature = "postgres")]
mod compensation;
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
mod lots;
#[cfg(feature = "postgres")]
mod operations_alert;
#[cfg(feature = "postgres")]
mod price_history;
#[cfg(feature = "postgres")]
mod public_equipment_link;
#[cfg(feature = "postgres")]
mod realtime;
#[cfg(feature = "postgres")]
mod realtime_admin;
#[cfg(feature = "postgres")]
mod realtime_consumer;
#[cfg(feature = "postgres")]
pub use realtime_consumer::{RealtimeConsumerOwnership, RealtimeConsumerOwnershipError};
#[cfg(feature = "postgres")]
#[path = "recovery_commands/commands.rs"]
mod recovery_commands;
#[cfg(feature = "postgres")]
mod recovery_health;
#[cfg(feature = "postgres")]
mod recovery_report;
#[cfg(feature = "postgres")]
#[path = "recovery_scheduler/scheduler.rs"]
mod recovery_scheduler;
#[cfg(feature = "postgres")]
mod retention_inventory;
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
pub(crate) use pool::TrackedPool;

mod backend;
#[cfg(feature = "postgres")]
mod postgres_store;
#[cfg(feature = "postgres")]
mod store;

pub use backend::{DatabaseBackend, StorageError};
#[cfg(feature = "postgres")]
pub(crate) use pool::PostgresPoolMetricsSource;
#[cfg(feature = "postgres")]
pub use postgres_store::{PostgresStore, postgres_expected_migration_version};
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
        assert!(DatabaseBackend::from_url("sqlite::memory:").is_err());
    }

    #[test]
    fn rejects_unknown_database_urls_without_echoing_credentials() {
        let error = DatabaseBackend::from_url("mysql://user:secret@example/db").unwrap_err();

        assert!(matches!(error, StorageError::UnsupportedScheme(ref scheme) if scheme == "mysql"));
        assert!(!error.to_string().contains("secret"));
    }

    // SQLite 连接测试仅保留历史源码；已移除驱动，不再注册为当前测试目标。
    #[cfg(any())]
    #[tokio::test]
    async fn connects_to_sqlite_memory_database() {
        let store = DatabaseStore::connect("sqlite::memory:")
            .await
            .unwrap_or_else(|error| panic!("SQLite 内存数据库应能连接: {error:?}"));

        assert_eq!(store.backend(), DatabaseBackend::Sqlite);
        assert!(store.sqlite_pool().is_some());
        #[cfg(feature = "postgres")]
        assert!(store.postgres_pool().is_none());
    }
}
