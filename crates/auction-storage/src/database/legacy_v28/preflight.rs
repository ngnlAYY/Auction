//! v28 只读接管预检与事实收集。

use super::mapping::*;
use super::source_checks::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use std::collections::BTreeSet;

use super::*;

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
}

pub(super) async fn collect_preflight(
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
