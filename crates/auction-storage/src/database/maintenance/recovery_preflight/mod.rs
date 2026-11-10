//! PostgreSQL 恢复前只读预检；只做同一只读事务中的编排，证据查询和结果模型位于子模块，
//! 预检不执行恢复、不建删数据库、不改变任何数据。

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::database::{DatabaseStore, PostgresMigrationInventory, PostgresSchemaIdentity};

mod catalog;
mod model;
mod query;

pub(crate) use catalog::USER_OBJECT_COUNT_SQL;

pub use model::{PostgresRecoveryPreflight, RECOVERY_PREFLIGHT_FORMAT, RecoveryPreflightError};
use query::{query_error, validate_database_name};
use query::{read_migration_inventory, read_schema_identity, set_read_only_limits};

impl DatabaseStore {
    /// 在只读、可重复读事务中收集恢复前预检证据；通过只表示目标库“空、无其他连接、主节点可写”。
    /// 它不能证明 backup archive、manifest、schema 兼容性或恢复后的业务数据正确，也不会执行恢复。
    pub async fn collect_recovery_preflight(
        &self,
        confirmed_database_name: &str,
    ) -> Result<PostgresRecoveryPreflight, RecoveryPreflightError> {
        validate_database_name(confirmed_database_name)?;
        let mut transaction = self
            .pool()
            .begin_read_snapshot()
            .await
            .map_err(|source| query_error("begin_read_only_transaction", source))?;
        set_read_only_limits(&mut transaction).await?;

        // `pg_user_mapping` 基表会拒绝普通数据库 owner 读取；公开目录视图保留完整行集合，只隐藏
        // 敏感 option，因此既能证明映射对象存在，又不要求 recovery 工具取得 superuser 权限。
        let identity = sqlx::query(
            "SELECT current_database() AS database_name,
                    current_setting('server_version') AS server_version,
                    transaction_timestamp() AS collected_at,
                    pg_is_in_recovery() AS in_recovery,
                    current_setting('default_transaction_read_only') = 'on'
                        AS default_transaction_read_only,
                    (SELECT COUNT(*)::bigint
                     FROM pg_stat_activity
                     WHERE datname = current_database()
                       AND pid <> pg_backend_pid()) AS active_other_sessions,
                    pg_database_size(current_database())::bigint AS database_size_bytes",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| query_error("database_state", source))?;
        let database_name: String = identity
            .try_get("database_name")
            .map_err(|source| query_error("database_state", source))?;
        if database_name != confirmed_database_name {
            return Err(RecoveryPreflightError::DatabaseMismatch {
                expected: confirmed_database_name.to_owned(),
                actual: database_name,
            });
        }
        let server_version: String = identity
            .try_get("server_version")
            .map_err(|source| query_error("database_state", source))?;
        let collected_at: DateTime<Utc> = identity
            .try_get("collected_at")
            .map_err(|source| query_error("database_state", source))?;
        let in_recovery: bool = identity
            .try_get("in_recovery")
            .map_err(|source| query_error("database_state", source))?;
        let default_transaction_read_only: bool = identity
            .try_get("default_transaction_read_only")
            .map_err(|source| query_error("database_state", source))?;
        let active_other_sessions: i64 = identity
            .try_get("active_other_sessions")
            .map_err(|source| query_error("database_state", source))?;
        let database_size_bytes: i64 = identity
            .try_get("database_size_bytes")
            .map_err(|source| query_error("database_state", source))?;
        let user_object_count: i64 = sqlx::query_scalar(USER_OBJECT_COUNT_SQL)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|source| query_error("user_objects", source))?;

        let schema_identity_present: bool =
            sqlx::query_scalar("SELECT to_regclass('public.auction_schema_identity') IS NOT NULL")
                .fetch_one(&mut *transaction)
                .await
                .map_err(|source| query_error("schema_marker", source))?;
        let schema_identity = if schema_identity_present {
            Some(read_schema_identity(&mut transaction).await?)
        } else {
            None
        };

        let migration_history_present: bool =
            sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
                .fetch_one(&mut *transaction)
                .await
                .map_err(|source| query_error("migration_marker", source))?;
        let migration = if migration_history_present {
            Some(read_migration_inventory(&mut transaction).await?)
        } else {
            None
        };

        let restore_write_possible = !in_recovery && !default_transaction_read_only;
        let restore_preflight_passed = restore_write_possible
            && active_other_sessions == 0
            && user_object_count == 0
            && !schema_identity_present
            && !migration_history_present;
        let mut notes = vec![
            "本预检只读取连接、数据库资源和 Auction schema 状态，不执行 pg_restore 或数据库变更"
                .to_owned(),
            "restore_preflight_passed=true 只表示空库/无其他连接/可写主节点基础条件满足".to_owned(),
        ];
        if !restore_write_possible {
            notes.push(
                "数据库处于恢复节点或 default_transaction_read_only，不能作为恢复目标".to_owned(),
            );
        }
        if active_other_sessions > 0 {
            notes.push("存在其他数据库连接；恢复前必须由外部部署编排停止角色和客户端".to_owned());
        }
        if user_object_count > 0 {
            notes.push("目标数据库存在用户对象；本命令不会覆盖或清理这些对象".to_owned());
        }
        if schema_identity_present || migration_history_present {
            notes.push("检测到 Auction schema 或迁移历史；本命令不会覆盖现有数据库".to_owned());
        }

        transaction.commit().await.map_err(|source| RecoveryPreflightError::Commit { source })?;

        Ok(PostgresRecoveryPreflight {
            format: RECOVERY_PREFLIGHT_FORMAT,
            collected_at,
            database_name,
            server_version,
            in_recovery,
            default_transaction_read_only,
            restore_write_possible,
            active_other_sessions,
            database_size_bytes,
            user_object_count,
            schema_identity_present,
            schema_identity,
            migration_history_present,
            migration,
            restore_preflight_passed,
            notes,
        })
    }
}
