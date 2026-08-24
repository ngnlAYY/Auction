//! Auction-BF PostgreSQL v28 的显式接管边界。
//!
//! 旧版使用 golang-migrate 的 `schema_migrations`，Rust 版使用压缩后的 SQLx 迁移树；两个
//! 版本号没有可直接比较的含义。本模块只接受精确、干净且停止外部连接的旧 v28，先验证
//! 关键事实可无损回填，再在同一事务内补齐 Rust v5 结构并写入受审计的 SQLx 基线。

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::{Connection, PgConnection, Row, migrate::Migrate};
use thiserror::Error;
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

use super::PostgresStore;

/// Auction-BF 最终发布的 PostgreSQL migration 版本。
pub const LEGACY_V28_MIGRATION_VERSION: i64 = 28;

/// 旧版 v28 接管预检的机器可读格式。
pub const POSTGRES_LEGACY_V28_PREFLIGHT_FORMAT: &str = "auction-postgres-legacy-v28-preflight.v1";

/// 旧 v28 已经等价覆盖的 Rust SQLx 迁移。
///
/// 1..=5 是压缩后的旧版最终 schema；v7 的 replay 唯一性在旧版 v25 已以另一索引名实现，
/// bridge 会补建当前索引，不能再执行 v7 中针对 fresh schema 临时约束的 DROP。
const BASELINED_SQLX_VERSIONS: &[i64] = &[1, 2, 3, 4, 5, 7];

const EXPECTED_LEGACY_TABLES: &[&str] = &[
    "admin_sessions",
    "auction_commands",
    "auction_forum_bid_commands",
    "auction_forum_reply_cursors",
    "auction_forum_sync",
    "auction_intake_targets",
    "auction_preview_items",
    "auction_schema_identity",
    "auction_settlements",
    "auction_submissions",
    "auctions",
    "audit_records",
    "bid_proxy_authorization_transitions",
    "bid_proxy_authorizations",
    "bids",
    "compensation_ledger",
    "credential_admin_commands",
    "credential_keys",
    "deliveries",
    "delivery_admin_commands",
    "equipment",
    "equipment_secrets",
    "forum_bidder_identities",
    "inbound_mail",
    "lot_settlements",
    "lots",
    "mail_admin_commands",
    "mail_commands",
    "mail_poll_state",
    "operations_alert_batch_items",
    "operations_alert_batches",
    "operations_alert_signals",
    "public_sessions",
    "realtime_consumer_offsets",
    "realtime_events",
    "realtime_failures",
    "realtime_streams",
    "rules_admin_commands",
    "rules_documents",
    "runtime_config_admin_commands",
    "scheduled_rounds",
    "schema_migrations",
    "service_heartbeats",
    "settlement_obligation_fulfillments",
    "settlement_obligations",
    "settlement_pools",
    "settlement_recoveries",
    "settlement_recovery_actions",
    "settlement_recovery_commands",
    "settlement_recovery_health",
    "user_admin_commands",
    "user_blacklist",
    "user_blacklist_history",
    "user_game_identity_history",
    "user_login_key_envelopes",
    "user_mail_commands",
    "user_renames",
    "users",
    "worlds",
];

/// 旧版 v28 接管的只读证据。
#[derive(Debug, Clone, Serialize)]
pub struct PostgresLegacyV28Preflight {
    pub format: &'static str,
    pub collected_at: DateTime<Utc>,
    pub database_name: String,
    pub server_version: String,
    pub server_version_num: i32,
    pub in_recovery: bool,
    pub default_transaction_read_only: bool,
    pub active_other_sessions: i64,
    pub sqlx_history_present: bool,
    pub legacy_history_present: bool,
    pub legacy_history_row_count: i64,
    pub legacy_version: Option<i64>,
    pub legacy_dirty: Option<bool>,
    pub schema_identity_valid: bool,
    pub expected_table_count: usize,
    pub actual_table_count: usize,
    pub missing_tables: Vec<String>,
    pub unexpected_tables: Vec<String>,
    pub source_shape_issue_count: i64,
    pub bridge_target_object_count: i64,
    pub policy_issue_count: i64,
    pub lot_projection_issue_count: i64,
    pub bid_event_issue_count: i64,
    pub adoption_ready: bool,
    pub notes: Vec<String>,
}

/// 一次成功接管的数据库侧结果；文件备份与不可覆盖 receipt 由 `auctionctl` 拥有。
#[derive(Debug, Clone, Serialize)]
pub struct PostgresLegacyV28Adoption {
    pub adopted_at: DateTime<Utc>,
    pub database_name: String,
    pub legacy_version: i64,
    pub baselined_sqlx_versions: Vec<i64>,
    pub final_sqlx_version: i64,
    pub audit_id: Uuid,
}

/// 旧版 v28 接管的安全错误分类。
#[derive(Debug, Error)]
pub enum PostgresLegacyV28Error {
    #[error("confirmed PostgreSQL database name is invalid")]
    InvalidDatabaseName,

    #[error("confirmed PostgreSQL database does not match the connected database")]
    DatabaseMismatch { expected: String, actual: String },

    #[error("legacy v28 adoption preflight did not pass")]
    PreflightRejected,

    #[error("failed to query legacy v28 adoption state at {stage}")]
    Query {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to acquire or release the SQLx migration advisory lock")]
    MigrationLock {
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    #[error("failed to apply the transactional legacy v28 bridge")]
    Bridge {
        #[source]
        source: sqlx::Error,
    },

    #[error(
        "legacy v28 bridge committed, but a later forward migration failed; rerun auctionctl migrate"
    )]
    ForwardMigration {
        #[source]
        source: sqlx::migrate::MigrateError,
    },

    #[error("embedded PostgreSQL migration baseline is incomplete")]
    EmbeddedBaselineIncomplete,
}

impl PostgresStore {
    /// 在只读、可重复读事务中检查当前连接是否是可安全接管的 Auction-BF v28。
    pub async fn collect_legacy_v28_preflight(
        &self,
        confirmed_database_name: &str,
    ) -> Result<PostgresLegacyV28Preflight, PostgresLegacyV28Error> {
        validate_database_name(confirmed_database_name)?;
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(|source| query_error("begin_read_only_transaction", source))?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await
            .map_err(|source| query_error("transaction_policy", source))?;
        sqlx::query("SET LOCAL statement_timeout = '10s'")
            .execute(&mut *transaction)
            .await
            .map_err(|source| query_error("statement_timeout", source))?;
        sqlx::query("SET LOCAL lock_timeout = '1s'")
            .execute(&mut *transaction)
            .await
            .map_err(|source| query_error("lock_timeout", source))?;

        let preflight = collect_preflight(&mut transaction, confirmed_database_name).await?;
        transaction
            .commit()
            .await
            .map_err(|source| query_error("commit_read_only_transaction", source))?;
        Ok(preflight)
    }

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

async fn collect_preflight(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    confirmed_database_name: &str,
) -> Result<PostgresLegacyV28Preflight, PostgresLegacyV28Error> {
    let identity = sqlx::query(
        "SELECT current_database() AS database_name,
                current_setting('server_version') AS server_version,
                current_setting('server_version_num')::integer AS server_version_num,
                transaction_timestamp() AS collected_at,
                pg_is_in_recovery() AS in_recovery,
                current_setting('default_transaction_read_only') = 'on'
                    AS default_transaction_read_only,
                (SELECT COUNT(*)::bigint
                 FROM pg_catalog.pg_stat_activity
                 WHERE datname = current_database() AND pid <> pg_backend_pid())
                    AS active_other_sessions",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("database_identity", source))?;
    let database_name: String = identity
        .try_get("database_name")
        .map_err(|source| query_error("database_identity", source))?;
    if database_name != confirmed_database_name {
        return Err(PostgresLegacyV28Error::DatabaseMismatch {
            expected: confirmed_database_name.to_owned(),
            actual: database_name,
        });
    }
    let server_version: String = identity
        .try_get("server_version")
        .map_err(|source| query_error("database_identity", source))?;
    let server_version_num: i32 = identity
        .try_get("server_version_num")
        .map_err(|source| query_error("database_identity", source))?;
    let collected_at: DateTime<Utc> = identity
        .try_get("collected_at")
        .map_err(|source| query_error("database_identity", source))?;
    let in_recovery: bool = identity
        .try_get("in_recovery")
        .map_err(|source| query_error("database_identity", source))?;
    let default_transaction_read_only: bool = identity
        .try_get("default_transaction_read_only")
        .map_err(|source| query_error("database_identity", source))?;
    let active_other_sessions: i64 = identity
        .try_get("active_other_sessions")
        .map_err(|source| query_error("database_identity", source))?;

    let actual_tables = sqlx::query_scalar::<_, String>(
        "SELECT class.relname
         FROM pg_catalog.pg_class AS class
         JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = class.relnamespace
         WHERE namespace.nspname = 'public' AND class.relkind IN ('r', 'p')
         ORDER BY class.relname",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("public_tables", source))?;
    let actual_table_set = actual_tables.iter().cloned().collect::<BTreeSet<_>>();
    let expected_table_set =
        EXPECTED_LEGACY_TABLES.iter().map(|name| (*name).to_owned()).collect::<BTreeSet<_>>();
    let missing_tables =
        expected_table_set.difference(&actual_table_set).cloned().collect::<Vec<_>>();
    let unexpected_tables =
        actual_table_set.difference(&expected_table_set).cloned().collect::<Vec<_>>();

    let sqlx_history_present = actual_table_set.contains("_sqlx_migrations");
    let legacy_history_present = actual_table_set.contains("schema_migrations");
    let (legacy_history_row_count, legacy_version, legacy_dirty) = if legacy_history_present {
        let history = sqlx::query(
            "SELECT COUNT(*)::bigint AS row_count,
                    MAX(version)::bigint AS version,
                    BOOL_OR(dirty) AS dirty
             FROM public.schema_migrations",
        )
        .fetch_one(&mut **transaction)
        .await
        .map_err(|source| query_error("legacy_migration_history", source))?;
        (
            history
                .try_get("row_count")
                .map_err(|source| query_error("legacy_migration_history", source))?,
            history
                .try_get("version")
                .map_err(|source| query_error("legacy_migration_history", source))?,
            history
                .try_get("dirty")
                .map_err(|source| query_error("legacy_migration_history", source))?,
        )
    } else {
        (0, None, None)
    };

    let schema_identity_valid = if actual_table_set.contains("auction_schema_identity") {
        sqlx::query_scalar::<_, bool>(
            "SELECT COUNT(*) = 1
                    AND BOOL_AND(singleton)
                    AND BOOL_AND(application = 'auction')
                    AND BOOL_AND(baseline_version = 1)
             FROM public.auction_schema_identity",
        )
        .fetch_one(&mut **transaction)
        .await
        .map_err(|source| query_error("schema_identity", source))?
    } else {
        false
    };

    let source_shape_issue_count = if missing_tables.is_empty() && unexpected_tables.is_empty() {
        critical_source_shape_issues(transaction).await?
    } else {
        1
    };
    let bridge_target_object_count = if missing_tables.is_empty() && unexpected_tables.is_empty() {
        bridge_target_object_count(transaction).await?
    } else {
        1
    };
    let policy_issue_count = if actual_table_set.contains("auctions") {
        policy_issue_count(transaction).await?
    } else {
        1
    };
    let lot_projection_issue_count =
        if actual_table_set.contains("lots") && actual_table_set.contains("bids") {
            lot_projection_issue_count(transaction).await?
        } else {
            1
        };
    let bid_event_issue_count =
        if actual_table_set.contains("bids") && actual_table_set.contains("realtime_events") {
            bid_event_issue_count(transaction).await?
        } else {
            1
        };

    let postgres_18 = (180_000..190_000).contains(&server_version_num);
    let clean_legacy_history = legacy_history_row_count == 1
        && legacy_version == Some(LEGACY_V28_MIGRATION_VERSION)
        && legacy_dirty == Some(false);
    let adoption_ready = postgres_18
        && !in_recovery
        && !default_transaction_read_only
        && active_other_sessions == 0
        && !sqlx_history_present
        && legacy_history_present
        && clean_legacy_history
        && schema_identity_valid
        && missing_tables.is_empty()
        && unexpected_tables.is_empty()
        && source_shape_issue_count == 0
        && bridge_target_object_count == 0
        && policy_issue_count == 0
        && lot_projection_issue_count == 0
        && bid_event_issue_count == 0;
    let mut notes = vec![
        "预检只读取旧版迁移标记、目录形状和可无损回填事实，不修改 schema 或数据".to_owned(),
        "adoption_ready=true 仍不替代已验证备份、停止进程和执行时二次确认".to_owned(),
    ];
    if !postgres_18 {
        notes.push("旧版 v28 的 Unicode casefold 索引要求 PostgreSQL 18.x".to_owned());
    }
    if active_other_sessions > 0 {
        notes.push("检测到其他数据库连接；必须先停止 Public/Admin/Worker 和其他客户端".to_owned());
    }
    if !clean_legacy_history {
        notes.push("schema_migrations 必须且只能包含 version=28、dirty=false".to_owned());
    }
    if !missing_tables.is_empty() || !unexpected_tables.is_empty() {
        notes.push("public table 集合不是 canonical Auction-BF v28，禁止猜测接管".to_owned());
    }
    if policy_issue_count > 0 || lot_projection_issue_count > 0 || bid_event_issue_count > 0 {
        notes.push("存在无法从旧 policy/lot/realtime 事实无损构造的 Rust 竞价投影".to_owned());
    }

    Ok(PostgresLegacyV28Preflight {
        format: POSTGRES_LEGACY_V28_PREFLIGHT_FORMAT,
        collected_at,
        database_name,
        server_version,
        server_version_num,
        in_recovery,
        default_transaction_read_only,
        active_other_sessions,
        sqlx_history_present,
        legacy_history_present,
        legacy_history_row_count,
        legacy_version,
        legacy_dirty,
        schema_identity_valid,
        expected_table_count: expected_table_set.len(),
        actual_table_count: actual_table_set.len(),
        missing_tables,
        unexpected_tables,
        source_shape_issue_count,
        bridge_target_object_count,
        policy_issue_count,
        lot_projection_issue_count,
        bid_event_issue_count,
        adoption_ready,
        notes,
    })
}

async fn critical_source_shape_issues(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "WITH expected_column(table_name, column_name, udt_name) AS (
             VALUES
               ('auctions', 'id', 'uuid'),
               ('auctions', 'world_id', 'text'),
               ('auctions', 'sequence', 'int8'),
               ('auctions', 'status', 'text'),
               ('auctions', 'preview_scheduled_at', 'timestamptz'),
               ('auctions', 'scheduled_open_at', 'timestamptz'),
               ('auctions', 'policy_snapshot', 'jsonb'),
               ('auctions', 'state_version', 'int8'),
               ('lots', 'id', 'uuid'),
               ('lots', 'auction_id', 'uuid'),
               ('lots', 'status', 'text'),
               ('lots', 'start_price', 'int8'),
               ('lots', 'current_price', 'int8'),
               ('lots', 'leader_user_id', 'uuid'),
               ('lots', 'leader_since', 'timestamptz'),
               ('lots', 'effective_end_at', 'timestamptz'),
               ('lots', 'slow_authorization_id', 'uuid'),
               ('lots', 'slow_due_at', 'timestamptz'),
               ('lots', 'slow_generation', 'int8'),
               ('lots', 'last_proxy_sequence', 'int8'),
               ('bids', 'id', 'uuid'),
               ('bids', 'command_id', 'uuid'),
               ('bids', 'fact_index', 'int4'),
               ('bids', 'lot_id', 'uuid'),
               ('bids', 'bidder_user_id', 'uuid'),
               ('bids', 'amount', 'int8'),
               ('bids', 'mechanism', 'text'),
               ('bids', 'accepted_state_version', 'int8'),
               ('bid_proxy_authorizations', 'authorization_id', 'uuid'),
               ('bid_proxy_authorizations', 'lot_id', 'uuid'),
               ('bid_proxy_authorizations', 'usage_number', 'int8'),
               ('bid_proxy_authorizations', 'mode', 'text'),
               ('bid_proxy_authorizations', 'status', 'text'),
               ('bid_proxy_authorization_transitions', 'transition_kind', 'text'),
               ('bid_proxy_authorization_transitions', 'old_status', 'text'),
               ('bid_proxy_authorization_transitions', 'new_status', 'text'),
               ('realtime_events', 'origin_bid_id', 'uuid'),
               ('realtime_events', 'origin_fact_index', 'int4'),
               ('realtime_events', 'public_payload', 'jsonb'),
               ('realtime_events', 'replay_of_event_id', 'uuid')
         ), missing_column AS (
             SELECT expected_column.column_name
             FROM expected_column
             LEFT JOIN information_schema.columns AS column
               ON column.table_schema = 'public'
              AND column.table_name = expected_column.table_name
              AND column.column_name = expected_column.column_name
              AND column.udt_name = expected_column.udt_name
             WHERE column.column_name IS NULL
         ), expected_constraint(table_name, constraint_name) AS (
             VALUES ('lots', 'lots_bidding_projection_v25_check')
         ), missing_constraint AS (
             SELECT expected_constraint.constraint_name
             FROM expected_constraint
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM pg_catalog.pg_constraint AS constraint
                 JOIN pg_catalog.pg_class AS class
                   ON class.oid = constraint.conrelid
                 JOIN pg_catalog.pg_namespace AS namespace
                   ON namespace.oid = class.relnamespace
                 WHERE namespace.nspname = 'public'
                   AND class.relname = expected_constraint.table_name
                   AND constraint.conname = expected_constraint.constraint_name
             )
         ), expected_index(index_name) AS (
             VALUES
               ('bid_proxy_authorizations_one_active_lot_idx'),
               ('bid_proxy_authorizations_lot_rank_idx'),
               ('users_username_case_insensitive_unique'),
               ('users_game_player_id_case_insensitive_unique'),
               ('forum_bidder_identities_casefold_unique')
         ), missing_index AS (
             SELECT expected_index.index_name
             FROM expected_index
             WHERE to_regclass(format('public.%I', expected_index.index_name)) IS NULL
         ), expected_trigger(table_name, trigger_name) AS (
             VALUES ('bids', 'bids_append_only')
         ), missing_trigger AS (
             SELECT expected_trigger.trigger_name
             FROM expected_trigger
             WHERE NOT EXISTS (
                 SELECT 1
                 FROM pg_catalog.pg_trigger AS trigger
                 JOIN pg_catalog.pg_class AS class ON class.oid = trigger.tgrelid
                 JOIN pg_catalog.pg_namespace AS namespace
                   ON namespace.oid = class.relnamespace
                 WHERE namespace.nspname = 'public'
                   AND class.relname = expected_trigger.table_name
                   AND trigger.tgname = expected_trigger.trigger_name
                   AND NOT trigger.tgisinternal
                   AND trigger.tgenabled = 'O'
             )
         )
         SELECT (
             (SELECT COUNT(*) FROM missing_column)
             + (SELECT COUNT(*) FROM missing_constraint)
             + (SELECT COUNT(*) FROM missing_index)
             + (SELECT COUNT(*) FROM missing_trigger)
         )::bigint",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("critical_source_shape", source))
}

async fn bridge_target_object_count(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "SELECT
             (SELECT COUNT(*)::bigint
              FROM information_schema.columns
              WHERE table_schema = 'public'
                AND (table_name, column_name) IN (
                    ('auctions', 'title'),
                    ('auctions', 'default_start_price'),
                    ('auctions', 'soft_close_window_seconds'),
                    ('auctions', 'soft_close_extension_seconds'),
                    ('auctions', 'cancel_cutoff_seconds'),
                    ('auctions', 'proxy_slow_interval_seconds'),
                    ('auctions', 'scheduled_end_at'),
                    ('auctions', 'effective_end_at'),
                    ('lots', 'leader_proxy_last'),
                    ('lots', 'material'),
                    ('bids', 'effective_end_at')
                ))
             + CASE WHEN to_regclass('public.bid_proxy_attempts') IS NULL THEN 0 ELSE 1 END
             + CASE WHEN to_regclass('public.bid_proxy_outbid_uses') IS NULL THEN 0 ELSE 1 END
             + CASE WHEN to_regclass('public.bid_proxies') IS NULL THEN 0 ELSE 1 END",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("bridge_target_objects", source))
}

async fn policy_issue_count(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "WITH candidate AS (
             SELECT
                 jsonb_typeof(policy_snapshot) = 'object' AS object_shape,
                 policy_snapshot ->> 'start_price' AS start_price,
                 policy_snapshot ->> 'soft_close_window_seconds' AS soft_close_window,
                 policy_snapshot ->> 'soft_close_extension_seconds' AS soft_close_extension,
                 policy_snapshot ->> 'proxy_slow_interval_seconds' AS proxy_slow_interval,
                 policy_snapshot ->> 'scheduled_end_at' AS scheduled_end_at
             FROM public.auctions
         ), validated AS (
             SELECT *,
                 COALESCE(pg_input_is_valid(start_price, 'bigint'), false) AS start_valid,
                 COALESCE(pg_input_is_valid(soft_close_window, 'bigint'), false)
                     AS window_valid,
                 COALESCE(pg_input_is_valid(soft_close_extension, 'bigint'), false)
                     AS extension_valid,
                 COALESCE(pg_input_is_valid(proxy_slow_interval, 'bigint'), false)
                     AS interval_valid,
                 COALESCE(pg_input_is_valid(scheduled_end_at, 'timestamptz'), false)
                     AS end_valid
             FROM candidate
         )
         SELECT COUNT(*)::bigint
         FROM validated
         WHERE object_shape IS DISTINCT FROM true
            OR NOT start_valid
            OR NOT window_valid
            OR NOT extension_valid
            OR NOT interval_valid
            OR NOT end_valid
            OR CASE WHEN start_valid THEN start_price::bigint <= 0 ELSE false END
            OR CASE WHEN window_valid THEN soft_close_window::bigint < 0 ELSE false END
            OR CASE WHEN extension_valid THEN soft_close_extension::bigint <= 0 ELSE false END
            OR CASE WHEN interval_valid THEN proxy_slow_interval::bigint <= 0 ELSE false END",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("policy_backfill", source))
}

async fn bid_event_issue_count(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*)::bigint
         FROM public.bids AS bid
         LEFT JOIN LATERAL (
             SELECT COUNT(*)::bigint AS event_count,
                    BOOL_AND(
                        jsonb_typeof(event.public_payload -> 'effective_end') = 'string'
                        AND pg_input_is_valid(
                            event.public_payload ->> 'effective_end', 'timestamptz')
                    ) AS valid_effective_end
             FROM public.realtime_events AS event
             WHERE event.kind = 'lot.bid_accepted'
               AND event.origin_bid_id = bid.id
               AND event.origin_fact_index = bid.fact_index
               AND event.replay_of_event_id IS NULL
         ) AS evidence ON true
         WHERE evidence.event_count <> 1
            OR evidence.valid_effective_end IS DISTINCT FROM true",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("bid_effective_end_evidence", source))
}

async fn lot_projection_issue_count(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<i64, PostgresLegacyV28Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*)::bigint
         FROM public.lots AS lot
         LEFT JOIN LATERAL (
             SELECT bid.bidder_user_id, bid.amount
             FROM public.bids AS bid
             WHERE bid.lot_id = lot.id
             ORDER BY bid.accepted_state_version DESC, bid.fact_index DESC, bid.id DESC
             LIMIT 1
         ) AS latest_bid ON true
         WHERE lot.current_price IS NULL
            OR lot.current_price < lot.start_price
            OR (
                lot.leader_user_id IS NULL
                AND (
                    lot.current_price <> lot.start_price
                    OR latest_bid.bidder_user_id IS NOT NULL
                )
            )
            OR (
                lot.leader_user_id IS NOT NULL
                AND (
                    latest_bid.bidder_user_id IS DISTINCT FROM lot.leader_user_id
                    OR latest_bid.amount IS DISTINCT FROM lot.current_price
                )
            )",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("lot_projection_evidence", source))
}

async fn require_locked_source(
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

fn query_error(stage: &'static str, source: sqlx::Error) -> PostgresLegacyV28Error {
    PostgresLegacyV28Error::Query { stage, source }
}

fn validate_database_name(value: &str) -> Result<(), PostgresLegacyV28Error> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 63
        || value.chars().any(char::is_control)
    {
        return Err(PostgresLegacyV28Error::InvalidDatabaseName);
    }
    Ok(())
}
