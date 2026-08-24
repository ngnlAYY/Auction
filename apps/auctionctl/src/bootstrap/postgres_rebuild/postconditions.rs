//! PostgreSQL rebuild 的源库约束、空库验证和迁移后置证明。

use anyhow::Context;
use auction_config::AppConfig;
use auction_storage::DatabaseStore;

use super::super::require_backup_source;

pub(super) fn require_rebuild_source(
    preflight: &auction_storage::PostgresRecoveryPreflight,
) -> anyhow::Result<i64> {
    if !preflight.restore_write_possible {
        anyhow::bail!("PostgreSQL rebuild source is read-only or a recovery replica");
    }
    if preflight.active_other_sessions != 0 {
        anyhow::bail!(
            "PostgreSQL rebuild source has {} other session(s); stop all roles and clients first",
            preflight.active_other_sessions,
        );
    }
    require_backup_source(preflight)
}

pub(super) async fn verify_empty_target(
    database_url: &str,
    database_name: &str,
) -> anyhow::Result<()> {
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect recreated PostgreSQL database for empty-target verification")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("recreated target no longer resolves to PostgreSQL")
        }
    };
    let preflight = postgres
        .collect_recovery_preflight(database_name)
        .await
        .context("collect recreated PostgreSQL empty-target evidence")?;
    super::super::postgres_restore::require_empty_restore_target(&preflight)?;
    postgres.close().await;
    Ok(())
}

pub(super) async fn migrate_and_verify(
    config: &AppConfig,
    database_name: &str,
) -> anyhow::Result<i64> {
    let database_url = config
        .database_url()
        .context("database_url disappeared after PostgreSQL rebuild migration")?;
    let migration_store = DatabaseStore::connect(database_url)
        .await
        .context("connect rebuilt PostgreSQL database for migration")?;
    migration_store.migrate().await.context("run rebuilt PostgreSQL database migrations")?;
    let migration_postgres = match migration_store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("rebuilt target no longer resolves to PostgreSQL")
        }
    };
    migration_postgres.close().await;
    println!("database migrations applied for postgres");

    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect rebuilt PostgreSQL database for migration verification")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("rebuilt target no longer resolves to PostgreSQL")
        }
    };
    let preflight = postgres
        .collect_recovery_preflight(database_name)
        .await
        .context("collect rebuilt PostgreSQL schema evidence")?;
    let version = require_backup_source(&preflight)?;
    if preflight.active_other_sessions != 0 {
        anyhow::bail!("rebuilt PostgreSQL target gained an unexpected external session");
    }
    postgres.close().await;
    Ok(version)
}
