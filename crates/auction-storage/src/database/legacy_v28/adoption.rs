//! v28 bridge、SQLx 基线写入与锁定源复核。

use super::mapping::*;
use chrono::Utc;
use serde_json::json;
use sqlx::{Connection, PgConnection, Row, migrate::Migrate};
use uuid::Uuid;

use super::*;

impl PostgresStore {
    /// 接管精确的 Auction-BF v28，并继续运行 Rust 尚未覆盖的前向迁移。
    ///
    /// 调用方必须先创建并离线验证旧库备份。该函数再次持有 SQLx migration advisory lock，
    /// 在 bridge 事务提交前不会暴露半套列或半套 migration history；bridge 之后的普通前向迁移
    /// 若失败，可以安全重跑 `auctionctl migrate`，不得回滚已经提交的数据语义转换。
    pub async fn adopt_legacy_v28(
        &self,
        confirmed_database_name: &str,
    ) -> Result<PostgresLegacyV28Adoption, PostgresLegacyV28Error> {
        let preflight = self.collect_legacy_v28_preflight(confirmed_database_name).await?;
        if !preflight.adoption_ready {
            return Err(PostgresLegacyV28Error::PreflightRejected);
        }

        let mut connection = self
            .raw_pool()
            .acquire()
            .await
            .map_err(|source| query_error("acquire_adoption_connection", source))?;
        Migrate::lock(&mut *connection)
            .await
            .map_err(|source| PostgresLegacyV28Error::MigrationLock { source })?;

        let result = async {
            require_locked_source(&mut connection, confirmed_database_name).await?;

            let mut transaction = connection
                .begin()
                .await
                .map_err(|source| PostgresLegacyV28Error::Bridge { source })?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
                .execute(&mut *transaction)
                .await
                .map_err(|source| PostgresLegacyV28Error::Bridge { source })?;
            sqlx::raw_sql(include_str!("bridge.sql"))
                .execute(&mut *transaction)
                .await
                .map_err(|source| PostgresLegacyV28Error::Bridge { source })?;

            sqlx::query(
                "CREATE TABLE public._sqlx_migrations (
                    version BIGINT PRIMARY KEY,
                    description TEXT NOT NULL,
                    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
                    success BOOLEAN NOT NULL,
                    checksum BYTEA NOT NULL,
                    execution_time BIGINT NOT NULL
                )",
            )
            .execute(&mut *transaction)
            .await
            .map_err(|source| PostgresLegacyV28Error::Bridge { source })?;

            let migrator = sqlx::migrate!("../../migrations/postgres");
            let mut inserted_baselines = 0_usize;
            for migration in migrator
                .iter()
                .filter(|migration| BASELINED_SQLX_VERSIONS.contains(&migration.version))
            {
                sqlx::query(
                    "INSERT INTO public._sqlx_migrations
                        (version, description, success, checksum, execution_time)
                     VALUES ($1, $2, true, $3, -1)",
                )
                .bind(migration.version)
                .bind(migration.description.as_ref())
                .bind(migration.checksum.as_ref())
                .execute(&mut *transaction)
                .await
                .map_err(|source| PostgresLegacyV28Error::Bridge { source })?;
                inserted_baselines += 1;
            }
            if inserted_baselines != BASELINED_SQLX_VERSIONS.len() {
                return Err(PostgresLegacyV28Error::EmbeddedBaselineIncomplete);
            }

            let audit_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO public.audit_records
                    (id, actor_type, actor_id, action, entity_type, entity_id, result,
                     request_id, source, metadata)
                 VALUES ($1, 'system', 'auctionctl', 'schema.legacy_v28_adopted',
                         'database', $2, 'success', $1::text, 'system', $3)",
            )
            .bind(audit_id)
            .bind(confirmed_database_name)
            .bind(json!({
                "legacy_migration_version": LEGACY_V28_MIGRATION_VERSION,
                "baselined_sqlx_versions": BASELINED_SQLX_VERSIONS,
            }))
            .execute(&mut *transaction)
            .await
            .record_audit_failure()
            .map_err(|source| PostgresLegacyV28Error::Bridge { source })?;

            transaction
                .commit()
                .await
                .map_err(|source| PostgresLegacyV28Error::Bridge { source })?;

            let mut forward_migrator = sqlx::migrate!("../../migrations/postgres");
            forward_migrator.set_locking(false);
            forward_migrator
                .run_direct(None, &mut *connection, false)
                .await
                .map_err(|source| PostgresLegacyV28Error::ForwardMigration { source })?;

            let final_sqlx_version = sqlx::query_scalar::<_, i64>(
                "SELECT COALESCE(MAX(version), 0)::bigint
                 FROM public._sqlx_migrations
                 WHERE success",
            )
            .fetch_one(&mut *connection)
            .await
            .map_err(|source| query_error("final_migration_version", source))?;
            Ok(PostgresLegacyV28Adoption {
                adopted_at: Utc::now(),
                database_name: confirmed_database_name.to_owned(),
                legacy_version: LEGACY_V28_MIGRATION_VERSION,
                baselined_sqlx_versions: BASELINED_SQLX_VERSIONS.to_vec(),
                final_sqlx_version,
                audit_id,
            })
        }
        .await;

        let unlock = Migrate::unlock(&mut *connection).await;
        if unlock.is_err() {
            // session advisory lock 可能仍由该物理连接持有；禁止把连接放回池中污染后续迁移。
            connection.close_on_drop();
        }
        match (result, unlock) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), _) => Err(error),
            (Ok(_), Err(source)) => Err(PostgresLegacyV28Error::MigrationLock { source }),
        }
    }
}

pub(super) async fn require_locked_source(
    connection: &mut PgConnection,
    confirmed_database_name: &str,
) -> Result<(), PostgresLegacyV28Error> {
    let row = sqlx::query(
        "SELECT current_database() AS database_name,
                current_setting('server_version_num')::integer AS server_version_num,
                pg_is_in_recovery() AS in_recovery,
                current_setting('default_transaction_read_only') = 'on' AS read_only,
                (SELECT COUNT(*)::bigint
                 FROM pg_catalog.pg_stat_activity
                 WHERE datname = current_database() AND pid <> pg_backend_pid())
                    AS active_other_sessions,
                to_regclass('public._sqlx_migrations') IS NOT NULL AS sqlx_present,
                to_regclass('public.schema_migrations') IS NOT NULL AS legacy_present",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|source| query_error("locked_source_identity", source))?;
    let actual_database: String = row
        .try_get("database_name")
        .map_err(|source| query_error("locked_source_identity", source))?;
    if actual_database != confirmed_database_name {
        return Err(PostgresLegacyV28Error::DatabaseMismatch {
            expected: confirmed_database_name.to_owned(),
            actual: actual_database,
        });
    }
    let server_version_num: i32 = row
        .try_get("server_version_num")
        .map_err(|source| query_error("locked_source_identity", source))?;
    let in_recovery: bool = row
        .try_get("in_recovery")
        .map_err(|source| query_error("locked_source_identity", source))?;
    let read_only: bool =
        row.try_get("read_only").map_err(|source| query_error("locked_source_identity", source))?;
    let active_other_sessions: i64 = row
        .try_get("active_other_sessions")
        .map_err(|source| query_error("locked_source_identity", source))?;
    let sqlx_present: bool = row
        .try_get("sqlx_present")
        .map_err(|source| query_error("locked_source_identity", source))?;
    let legacy_present: bool = row
        .try_get("legacy_present")
        .map_err(|source| query_error("locked_source_identity", source))?;
    if !(180_000..190_000).contains(&server_version_num)
        || in_recovery
        || read_only
        || active_other_sessions != 0
        || sqlx_present
        || !legacy_present
    {
        return Err(PostgresLegacyV28Error::PreflightRejected);
    }
    Ok(())
}
