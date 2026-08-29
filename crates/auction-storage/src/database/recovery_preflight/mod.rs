//! PostgreSQL 恢复前只读预检。
//!
//! 本模块只负责同一只读事务中的预检编排；结果模型、错误分类和数据库证据查询分别位于
//! 子模块，预检不会执行恢复、创建/删除数据库或改变任何数据。

use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{PostgresMigrationInventory, PostgresSchemaIdentity, PostgresStore};

mod model;
mod query;

pub use model::{PostgresRecoveryPreflight, RECOVERY_PREFLIGHT_FORMAT, RecoveryPreflightError};
use query::{query_error, validate_database_name};
use query::{read_migration_inventory, read_schema_identity, set_read_only_limits};

impl PostgresStore {
    /// 在只读、可重复读事务中收集恢复前预检证据。
    ///
    /// 预检通过只表示目标数据库满足“空、无其他连接、主节点可写”的基础条件，不能证明
    /// backup archive、manifest、schema 兼容性或恢复后的业务数据正确，也不会执行恢复。
    pub async fn collect_recovery_preflight(
        &self,
        confirmed_database_name: &str,
    ) -> Result<PostgresRecoveryPreflight, RecoveryPreflightError> {
        validate_database_name(confirmed_database_name)?;
        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(|source| query_error("begin_read_only_transaction", source))?;
        set_read_only_limits(&mut transaction).await?;

        // `pg_user_mapping` 基表会拒绝普通数据库 owner 读取；公开目录视图保留完整行集合，只隐藏
        // 敏感 option，因此既能证明映射对象存在，又不要求 recovery 工具取得 superuser 权限。
        let identity = sqlx::query(
            "WITH user_namespaces AS (
                 SELECT oid, nspname
                 FROM pg_catalog.pg_namespace
                 WHERE nspname NOT IN ('pg_catalog', 'information_schema')
                   AND nspname !~ '^pg_(toast|temp)'
             ),
             user_objects AS (
                 SELECT oid FROM user_namespaces WHERE nspname <> 'public'
                 UNION ALL
                 SELECT class_record.oid
                 FROM pg_catalog.pg_class AS class_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = class_record.relnamespace
                 UNION ALL
                 SELECT type_record.oid
                 FROM pg_catalog.pg_type AS type_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = type_record.typnamespace
                 UNION ALL
                 SELECT procedure_record.oid
                 FROM pg_catalog.pg_proc AS procedure_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = procedure_record.pronamespace
                 UNION ALL
                 SELECT collation_record.oid
                 FROM pg_catalog.pg_collation AS collation_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = collation_record.collnamespace
                 UNION ALL
                 SELECT conversion_record.oid
                 FROM pg_catalog.pg_conversion AS conversion_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = conversion_record.connamespace
                 UNION ALL
                 SELECT operator_record.oid
                 FROM pg_catalog.pg_operator AS operator_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = operator_record.oprnamespace
                 UNION ALL
                 SELECT operator_class_record.oid
                 FROM pg_catalog.pg_opclass AS operator_class_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = operator_class_record.opcnamespace
                 UNION ALL
                 SELECT operator_family_record.oid
                 FROM pg_catalog.pg_opfamily AS operator_family_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = operator_family_record.opfnamespace
                 UNION ALL
                 SELECT configuration_record.oid
                 FROM pg_catalog.pg_ts_config AS configuration_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = configuration_record.cfgnamespace
                 UNION ALL
                 SELECT dictionary_record.oid
                 FROM pg_catalog.pg_ts_dict AS dictionary_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = dictionary_record.dictnamespace
                 UNION ALL
                 SELECT parser_record.oid
                 FROM pg_catalog.pg_ts_parser AS parser_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = parser_record.prsnamespace
                 UNION ALL
                 SELECT template_record.oid
                 FROM pg_catalog.pg_ts_template AS template_record
                 JOIN user_namespaces AS namespace_record
                   ON namespace_record.oid = template_record.tmplnamespace
                 UNION ALL
                 SELECT extension_record.oid
                 FROM pg_catalog.pg_extension AS extension_record
                 WHERE extension_record.extname <> 'plpgsql'
                 UNION ALL
                 SELECT wrapper_record.oid
                 FROM pg_catalog.pg_foreign_data_wrapper AS wrapper_record
                 UNION ALL
                 SELECT server_record.oid
                 FROM pg_catalog.pg_foreign_server AS server_record
                 UNION ALL
                 SELECT mapping_record.umid
                 FROM pg_catalog.pg_user_mappings AS mapping_record
                 UNION ALL
                 SELECT trigger_record.oid
                 FROM pg_catalog.pg_event_trigger AS trigger_record
                 UNION ALL
                 SELECT publication_record.oid
                 FROM pg_catalog.pg_publication AS publication_record
                 UNION ALL
                 SELECT subscription_record.oid
                 FROM pg_catalog.pg_subscription AS subscription_record
                 UNION ALL
                 SELECT object_record.oid
                 FROM pg_catalog.pg_largeobject_metadata AS object_record
                 UNION ALL
                 SELECT acl_record.oid
                 FROM pg_catalog.pg_default_acl AS acl_record
             )
             SELECT current_database() AS database_name,
                    current_setting('server_version') AS server_version,
                    transaction_timestamp() AS collected_at,
                    pg_is_in_recovery() AS in_recovery,
                    current_setting('default_transaction_read_only') = 'on'
                        AS default_transaction_read_only,
                    (SELECT COUNT(*)::bigint
                     FROM pg_stat_activity
                     WHERE datname = current_database()
                       AND pid <> pg_backend_pid()) AS active_other_sessions,
                    pg_database_size(current_database())::bigint AS database_size_bytes,
                    (SELECT COUNT(*)::bigint FROM user_objects) AS user_object_count",
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
        let user_object_count: i64 = identity
            .try_get("user_object_count")
            .map_err(|source| query_error("database_state", source))?;

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
