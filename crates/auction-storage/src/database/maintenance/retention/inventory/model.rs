//! retention 盘点对外模型与失败分类；只描述安全投影，不代表归档、删除或资格授权。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 只读盘点使用的版本化保留政策身份。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetentionPolicy {
    pub format: String,
    pub policy_id: String,
    pub version: i64,
    pub database_schema_version: i64,
    pub authority: String,
    pub inventory_authorizes_mutation: bool,
    #[serde(default)]
    pub deletion_requires_dry_run_and_operator_approval: bool,
    #[serde(default)]
    pub audit_financial_and_business_facts_retain_forever: bool,
    #[serde(default)]
    pub restore_objectives: Option<RetentionRestoreObjectives>,
    pub families: Vec<RetentionPolicyFamily>,
    #[serde(default)]
    pub resolved_operator_decisions: Vec<String>,
    pub required_operator_decisions: Vec<String>,
}

/// 恢复目标与演练节奏（决策 31）；仅作为政策事实记录，不驱动备份执行。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetentionRestoreObjectives {
    pub rpo_seconds: i64,
    pub rto_seconds: i64,
    pub wal_archive_enabled: bool,
    pub point_in_time_recovery_window_days: i64,
    pub rehearsal_cadence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetentionPolicyFamily {
    pub dataset_family: String,
    pub feature_owner: String,
    pub policy_state: String,
    #[serde(default)]
    pub conservative_behavior_until_decided: Option<String>,
    pub candidate_class: String,
    pub mutation_authorized: bool,
    pub tables: Vec<String>,
    /// 只有运行态数据族携带清理规则；`retain_forever` 族必须省略。
    #[serde(default)]
    pub retention: Option<RetentionPolicyRetention>,
}

/// 一个数据族的清理规则集合；`rule_id` 使用代码内闭集，政策只提供期限。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetentionPolicyRetention {
    pub kind: String,
    pub rules: Vec<RetentionPolicyRule>,
    #[serde(default)]
    pub retain_forever: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetentionPolicyRule {
    pub rule_id: String,
    #[serde(default)]
    pub table: Option<String>,
    pub basis: String,
    pub duration_days: i64,
}

/// PostgreSQL schema 身份安全投影。
#[derive(Debug, Serialize)]
pub struct PostgresSchemaIdentity {
    pub application: String,
    pub baseline_version: i64,
    pub installed_at: DateTime<Utc>,
}

/// SQLx 迁移历史安全投影。
#[derive(Debug, Serialize)]
pub struct PostgresMigrationInventory {
    pub migration_count: i64,
    pub latest_version: i64,
    pub latest_description: Option<String>,
    pub all_success: bool,
    pub expected_version: i64,
    pub current: bool,
}

/// PostgreSQL public 关系的大小与行数估计。
#[derive(Debug, Serialize)]
pub struct PostgresRelationInventory {
    pub schema_name: String,
    pub relation_name: String,
    pub relation_kind: String,
    pub estimated_rows: i64,
    pub estimated_dead_rows: i64,
    pub catalog_rows_estimate: i64,
    pub last_analyze: Option<DateTime<Utc>>,
    pub last_autoanalyze: Option<DateTime<Utc>>,
    pub table_bytes: i64,
    pub index_bytes: i64,
    pub total_bytes: i64,
}

/// 保留政策中一个目标表的覆盖与统计证据。
#[derive(Debug, Serialize)]
pub struct PostgresRetentionTableInventory {
    pub dataset_family: String,
    pub feature_owner: String,
    pub policy_state: String,
    pub candidate_class: String,
    pub table_name: String,
    pub relation_present: bool,
    pub estimated_rows: Option<i64>,
    pub estimated_dead_rows: Option<i64>,
    pub catalog_rows_estimate: Option<i64>,
    pub last_analyze: Option<DateTime<Utc>>,
    pub last_autoanalyze: Option<DateTime<Utc>>,
    pub table_bytes: Option<i64>,
    pub index_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    /// 固定为 false；表统计不能转换为保留资格。
    pub eligibility_assessed: bool,
    /// 固定为 false；政策矩阵不授予归档或删除权限。
    pub eligibility_authorized: bool,
}

/// 一个数据族的有限统计汇总。
#[derive(Debug, Serialize)]
pub struct PostgresRetentionFamilyInventory {
    pub dataset_family: String,
    pub feature_owner: String,
    pub policy_state: String,
    pub candidate_class: String,
    pub configured_table_count: i64,
    pub present_table_count: i64,
    pub estimated_rows: i64,
    pub estimated_dead_rows: i64,
    pub total_bytes: i64,
    pub statistics_complete: bool,
    /// 固定为 false；族汇总不是资格判断。
    pub eligibility_assessed: bool,
    /// 固定为 false；族汇总不是变更授权。
    pub eligibility_authorized: bool,
}

/// 一个 realtime stream 的安全水位摘要。
#[derive(Debug, Serialize)]
pub struct PostgresRealtimeStreamInventory {
    pub stream_key: String,
    pub min_sequence: Option<i64>,
    pub max_sequence: Option<i64>,
    pub event_count: i64,
    pub consumer_count: i64,
    pub diagnostic_min_consumer_offset: Option<i64>,
    pub events_below_diagnostic_min_offset: i64,
    pub unresolved_failure_min_sequence: Option<i64>,
    pub unresolved_failure_max_sequence: Option<i64>,
    pub watermark_kind: &'static str,
    pub eligibility_assessed: bool,
    pub eligibility_authorized: bool,
}

#[derive(Debug, Serialize)]
pub struct PostgresRealtimeCoverage {
    pub configured_stream_count: i64,
    pub reported_stream_count: i64,
    pub stream_rows_truncated: bool,
    pub configured_consumer_offset_count: i64,
    pub diagnostic_consumer_offset_count: i64,
    pub eligibility_assessed: bool,
    pub eligibility_authorized: bool,
}

#[derive(Debug, Serialize)]
pub struct PostgresRealtimeInventory {
    pub streams: Vec<PostgresRealtimeStreamInventory>,
    pub coverage: PostgresRealtimeCoverage,
}

/// 一个 public-schema 外键关系的安全摘要。
#[derive(Debug, Serialize)]
pub struct PostgresForeignKeyInventory {
    pub source_table: String,
    pub referenced_table: String,
    pub constraint_name: String,
    pub delete_action: String,
    pub dependency_is_eligibility_blocker_until_policy_defined: bool,
    pub eligibility_assessed: bool,
    pub eligibility_authorized: bool,
}

#[derive(Debug, Serialize)]
pub struct PostgresForeignKeyCoverage {
    pub foreign_key_count: i64,
    pub reported_foreign_key_count: i64,
    pub foreign_key_rows_truncated: bool,
    pub eligibility_assessed: bool,
    pub eligibility_authorized: bool,
}

#[derive(Debug, Serialize)]
pub struct PostgresForeignKeyInventoryReport {
    pub relations: Vec<PostgresForeignKeyInventory>,
    pub coverage: PostgresForeignKeyCoverage,
}

#[derive(Debug, Serialize)]
pub struct PostgresRetentionInventory {
    pub format: &'static str,
    pub collected_at: DateTime<Utc>,
    pub database_name: String,
    pub server_version: String,
    pub server_version_num: String,
    pub transaction_snapshot: String,
    pub transaction_read_only: String,
    pub statistics_reset_at: Option<DateTime<Utc>>,
    pub policy_sha256: String,
    pub policy: RetentionPolicy,
    pub schema_identity: PostgresSchemaIdentity,
    pub migration: PostgresMigrationInventory,
    pub relation_count: i64,
    pub relations: Vec<PostgresRelationInventory>,
    pub policy_table_count: i64,
    pub policy_tables: Vec<PostgresRetentionTableInventory>,
    pub policy_families: Vec<PostgresRetentionFamilyInventory>,
    pub realtime: PostgresRealtimeInventory,
    pub foreign_keys: PostgresForeignKeyInventoryReport,
    pub missing_operator_decisions: Vec<String>,
    /// 固定为 false；盘点不评估法律、业务或依赖关系上的保留资格。
    pub eligibility_assessed: bool,
    /// 固定为 false；盘点不授予归档或删除权限。
    pub eligibility_authorized: bool,
    pub notes: Vec<String>,
}

/// 只读盘点失败的安全错误分类。
#[derive(Debug, Error)]
pub enum RetentionInventoryError {
    #[error("confirmed PostgreSQL database name is invalid")]
    InvalidDatabaseName,

    #[error("confirmed PostgreSQL database does not match the connected database")]
    DatabaseMismatch { expected: String, actual: String },

    #[error("Auction migration history is not present")]
    MigrationHistoryMissing,

    #[error("Auction schema identity is missing or invalid")]
    SchemaIdentityInvalid,

    #[error("versioned retention policy is missing or invalid")]
    PolicyInvalid,

    #[error("retention inventory transaction is not read-only")]
    ReadOnlyTransactionInvalid,

    #[error("PostgreSQL public relation count exceeds the safe evidence limit of {0}")]
    TooManyRelations(i64),

    #[error("retention inventory count exceeds the representable range")]
    CountOverflow,

    #[error("failed to query PostgreSQL retention inventory at {stage}")]
    Query {
        stage: &'static str,
        #[source]
        source: sqlx::Error,
    },

    #[error("failed to finish PostgreSQL retention inventory transaction")]
    Commit {
        #[source]
        source: sqlx::Error,
    },
}
