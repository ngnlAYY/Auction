#[cfg(any(feature = "postgres", feature = "sqlite"))]
use std::sync::Arc;
#[cfg(feature = "sqlite")]
use std::time::Duration;

#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_application::{ApplicationError, ProcessReadinessPort, RepositoryError};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_observability::{HeartbeatHealthReader, HeartbeatReadinessFuture, PoolMetricsSource};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::{PgPool, Postgres, postgres::PgPoolOptions};
#[cfg(feature = "postgres")]
mod admin_intake;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_mail;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod audit_health;
#[cfg(feature = "postgres")]
mod bootstrap;
#[cfg(feature = "postgres")]
#[path = "competition/postgres.rs"]
mod competition;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod competition_audit;
#[cfg(feature = "postgres")]
mod legacy_v28;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod lifecycle_audit;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod mail_delivery;
#[cfg(feature = "postgres")]
mod mail_intake;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod mail_poll;
#[cfg(feature = "postgres")]
mod migrations;
#[cfg(feature = "postgres")]
mod rebuild;
#[cfg(feature = "postgres")]
mod recovery_preflight;
#[cfg(feature = "postgres")]
mod settlement;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod settlement_report;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
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
#[cfg(feature = "sqlite")]
mod sqlite;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteCompetitionTransaction;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod commands;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod compensation;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod credentials;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub use commands::{
    DatabaseAdminAuctionCommands, DatabaseAdminIntakeCommands, DatabaseAdminLotCommands,
    DatabaseAdminMailCommands, DatabaseAdminRulesCommands, DatabaseAdminRuntimeCommands,
    DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands,
};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod account;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_auctions;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_audit;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_lots;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_proxy_events;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod admin_renames;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod auctions;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod bid_log;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod blacklist;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod delivery_admin;
#[cfg(feature = "postgres")]
mod delivery_audit;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod delivery_scheduler;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod forum;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod fulfillment_scheduler;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod health;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod lots;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod operations_alert;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod price_history;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod realtime;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod realtime_admin;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod realtime_consumer;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[path = "recovery_commands/commands.rs"]
mod recovery_commands;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod recovery_health;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod recovery_report;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[path = "recovery_scheduler/scheduler.rs"]
mod recovery_scheduler;
#[cfg(feature = "postgres")]
mod retention_inventory;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod rules;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod scheduler;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod sessions;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod settlement_commands;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod settlement_scheduler;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub use settlement_commands::DatabaseSettlementCommands;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod users;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod worlds;
#[cfg(feature = "sqlite")]
use sqlx::{
    Sqlite, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod pool;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
pub(crate) use pool::TrackedPool;

mod backend;
#[cfg(feature = "postgres")]
mod postgres_store;
#[cfg(feature = "sqlite")]
mod sqlite_store;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
mod store;

pub use backend::{DatabaseBackend, StorageError};
#[cfg(feature = "postgres")]
pub(crate) use pool::PostgresPoolMetricsSource;
#[cfg(feature = "sqlite")]
pub(crate) use pool::SqlitePoolMetricsSource;
#[cfg(feature = "postgres")]
pub use postgres_store::{PostgresStore, postgres_expected_migration_version};
#[cfg(feature = "sqlite")]
pub use sqlite_store::SqliteStore;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
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
        assert_eq!(DatabaseBackend::from_url("sqlite::memory:").unwrap(), DatabaseBackend::Sqlite);
    }

    #[test]
    fn rejects_unknown_database_urls_without_echoing_credentials() {
        let error = DatabaseBackend::from_url("mysql://user:secret@example/db").unwrap_err();

        assert!(matches!(error, StorageError::UnsupportedScheme(ref scheme) if scheme == "mysql"));
        assert!(!error.to_string().contains("secret"));
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn connects_to_sqlite_memory_database() {
        let store = DatabaseStore::connect("sqlite::memory:").await.unwrap();

        assert_eq!(store.backend(), DatabaseBackend::Sqlite);
        assert!(store.sqlite_pool().is_some());
        #[cfg(feature = "postgres")]
        assert!(store.postgres_pool().is_none());
    }
}
