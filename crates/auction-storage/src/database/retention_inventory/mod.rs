//! PostgreSQL 只读保留盘点。
//!
//! 该模块只读取同一 `REPEATABLE READ`、只读事务中的数据库身份、迁移状态、schema 身份和
//! 关系大小/行数估计。它不判断任何记录是否可以归档或删除，也不提供写入、归档、恢复或
//! retention mutation 能力；盘点结果只能作为后续人工政策评审的输入。

use chrono::{DateTime, Utc};
use sqlx::Row;

use super::PostgresStore;

/// 盘点证据的机器可读格式标识。
pub const RETENTION_INVENTORY_FORMAT: &str = "auction-retention-inventory.v1";

/// 编译进运维制品的保留政策矩阵。
const RETENTION_POLICY_JSON: &str =
    include_str!("../../../../../ops/deploy/retention-policy.v1.json");

/// 单次盘点最多报告的 public 关系数量。
const MAX_RELATIONS: i64 = 512;

/// 超过该边界时失败关闭，而不是静默截断证据。
const RELATION_QUERY_LIMIT: i64 = MAX_RELATIONS + 1;

/// realtime 盘点最多输出的 stream 数量；超出部分只记录覆盖标记。
const MAX_REALTIME_STREAMS: i64 = 128;
const REALTIME_STREAM_QUERY_LIMIT: i64 = MAX_REALTIME_STREAMS + 1;

/// 外键目录证据的最大输出行数；超出部分只记录覆盖标记。
const MAX_FOREIGN_KEYS: i64 = 256;
const FOREIGN_KEY_QUERY_LIMIT: i64 = MAX_FOREIGN_KEYS + 1;

mod catalog;
mod mapping;
mod model;
mod policy;
mod realtime;

use catalog::*;
use mapping::*;
pub use model::*;
use policy::*;
use realtime::*;

impl PostgresStore {
    /// 在只读、可重复读事务中收集有限的 PostgreSQL retention 盘点。
    ///
    /// `confirmed_database_name` 是人为确认的目标数据库名，必须与连接实际返回的
    /// `current_database()` 完全一致。该方法不会执行迁移、锁定业务行、写入证据或改变任何
    /// 数据；调用方负责把返回值以不可覆盖的私有文件保存。
    pub async fn collect_retention_inventory(
        &self,
        confirmed_database_name: &str,
    ) -> Result<PostgresRetentionInventory, RetentionInventoryError> {
        validate_database_name(confirmed_database_name)?;
        let policy = load_retention_policy()?;
        let expected_version = super::postgres_expected_migration_version();
        if policy.database_schema_version != expected_version {
            return Err(RetentionInventoryError::PolicyInvalid);
        }

        let mut transaction = self
            .pool()
            .begin()
            .await
            .map_err(|source| query_error("begin_read_only_transaction", source))?;
        set_read_only_limits(&mut transaction).await?;

        let identity = sqlx::query(
            "SELECT current_database() AS database_name,
                    current_setting('server_version') AS server_version,
                    current_setting('server_version_num') AS server_version_num,
                    transaction_timestamp() AS collected_at,
                    txid_current_snapshot()::text AS transaction_snapshot,
                    current_setting('transaction_read_only') AS transaction_read_only,
                    (SELECT stats_reset
                     FROM pg_stat_database
                     WHERE datname = current_database()) AS statistics_reset_at",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| query_error("database_identity", source))?;
        let database_name = identity_string(&identity, "database_name", "database_identity")?;
        if database_name != confirmed_database_name {
            return Err(RetentionInventoryError::DatabaseMismatch {
                expected: confirmed_database_name.to_owned(),
                actual: database_name,
            });
        }
        let server_version = identity_string(&identity, "server_version", "database_identity")?;
        let server_version_num =
            identity_string(&identity, "server_version_num", "database_identity")?;
        let transaction_snapshot =
            identity_string(&identity, "transaction_snapshot", "database_identity")?;
        let transaction_read_only =
            identity_string(&identity, "transaction_read_only", "database_identity")?;
        if transaction_read_only != "on" {
            return Err(RetentionInventoryError::ReadOnlyTransactionInvalid);
        }
        let statistics_reset_at: Option<DateTime<Utc>> = identity
            .try_get("statistics_reset_at")
            .map_err(|source| query_error("database_identity", source))?;
        let collected_at = identity
            .try_get("collected_at")
            .map_err(|source| query_error("database_identity", source))?;

        let marker_exists = sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| query_error("migration_marker", source))?;
        if !marker_exists {
            return Err(RetentionInventoryError::MigrationHistoryMissing);
        }

        let schema_identity = read_schema_identity(&mut transaction).await?;
        let migration = read_migration_inventory(&mut transaction).await?;
        let relations = read_relations(&mut transaction).await?;
        let policy_tables = build_policy_table_inventory(&policy, &relations);
        let policy_families = build_policy_family_inventory(&policy, &policy_tables)?;
        let realtime = read_realtime_inventory(&mut transaction).await?;
        let foreign_keys = read_foreign_key_inventory(&mut transaction).await?;
        let relation_count =
            i64::try_from(relations.len()).map_err(|_| RetentionInventoryError::CountOverflow)?;
        let policy_table_count = i64::try_from(policy_tables.len())
            .map_err(|_| RetentionInventoryError::CountOverflow)?;
        let mut notes = vec![
            "本盘点只读取 PostgreSQL 系统目录、schema 身份、迁移历史和 pg_stat_user_tables 估计值".to_owned(),
            "policy_tables.relation_present=false 表示政策声明的目标表在当前 schema 中缺失".to_owned(),
            "realtime 的 consumer 最小 offset 只是诊断水位，不是可归档的权威安全水位".to_owned(),
            "foreign_keys 的依赖关系在政策明确前一律视为资格阻断因素".to_owned(),
            "eligibility_assessed=false；结果不是归档或删除授权".to_owned(),
            "relations 与 policy_tables 的行数和体积统计可能因 PostgreSQL 统计信息延迟或重置而不精确".to_owned(),
        ];
        if !migration.current {
            notes.push(
                "迁移历史不是当前编译制品的完整干净版本；禁止把该证据用于切换授权".to_owned(),
            );
        }

        transaction.commit().await.map_err(|source| RetentionInventoryError::Commit { source })?;

        Ok(PostgresRetentionInventory {
            format: RETENTION_INVENTORY_FORMAT,
            collected_at,
            database_name,
            server_version,
            server_version_num,
            transaction_snapshot,
            transaction_read_only,
            statistics_reset_at,
            policy_sha256: sha256_hex(RETENTION_POLICY_JSON.as_bytes()),
            missing_operator_decisions: policy.required_operator_decisions.clone(),
            policy,
            schema_identity,
            migration,
            relation_count,
            relations,
            policy_table_count,
            policy_tables,
            policy_families,
            realtime,
            foreign_keys,
            eligibility_assessed: false,
            eligibility_authorized: false,
            notes,
        })
    }
}
