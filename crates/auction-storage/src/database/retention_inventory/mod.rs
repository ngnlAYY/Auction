//! PostgreSQL 只读保留盘点。
//!
//! 该模块只读取同一 `REPEATABLE READ`、只读事务中的数据库身份、迁移状态、schema 身份和
//! 关系大小/行数估计。它不判断任何记录是否可以归档或删除，也不提供写入、归档、恢复或
//! retention mutation 能力；盘点结果只能作为后续人工政策评审的输入。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, postgres::PgRow};
use std::collections::BTreeMap;
use thiserror::Error;

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

/// 只读盘点使用的版本化保留政策身份。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetentionPolicy {
    pub format: String,
    pub policy_id: String,
    pub version: i64,
    pub database_schema_version: i64,
    pub authority: String,
    pub inventory_authorizes_mutation: bool,
    pub families: Vec<RetentionPolicyFamily>,
    pub required_operator_decisions: Vec<String>,
}

/// 保留政策中的数据族定义。
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

/// realtime 盘点的输出覆盖信息。
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

/// realtime 盘点结果。
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

/// public-schema 外键证据及其覆盖信息。
#[derive(Debug, Serialize)]
pub struct PostgresForeignKeyCoverage {
    pub foreign_key_count: i64,
    pub reported_foreign_key_count: i64,
    pub foreign_key_rows_truncated: bool,
    pub eligibility_assessed: bool,
    pub eligibility_authorized: bool,
}

/// 外键依赖盘点结果。
#[derive(Debug, Serialize)]
pub struct PostgresForeignKeyInventoryReport {
    pub relations: Vec<PostgresForeignKeyInventory>,
    pub coverage: PostgresForeignKeyCoverage,
}

/// 一次只读保留盘点结果。
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
        let policy_families = build_policy_family_inventory(&policy, &policy_tables);
        let realtime = read_realtime_inventory(&mut transaction).await?;
        let foreign_keys = read_foreign_key_inventory(&mut transaction).await?;
        let relation_count = i64::try_from(relations.len()).unwrap_or(MAX_RELATIONS);
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
            policy_table_count: i64::try_from(policy_tables.len()).unwrap_or(i64::MAX),
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

fn load_retention_policy() -> Result<RetentionPolicy, RetentionInventoryError> {
    let policy: RetentionPolicy = serde_json::from_str(RETENTION_POLICY_JSON)
        .map_err(|_| RetentionInventoryError::PolicyInvalid)?;
    if policy.format != "auction-retention-policy.v1"
        || policy.policy_id != "auction-retention-policy"
        || policy.version != 1
        || policy.authority != "repository-conservative-default"
        || policy.inventory_authorizes_mutation
        || policy.families.is_empty()
        || policy.required_operator_decisions.is_empty()
    {
        return Err(RetentionInventoryError::PolicyInvalid);
    }

    let mut seen_tables = BTreeMap::new();
    for family in &policy.families {
        if family.dataset_family.is_empty()
            || family.feature_owner.is_empty()
            || family.policy_state.is_empty()
            || family.candidate_class.is_empty()
            || family.mutation_authorized
            || family.tables.is_empty()
        {
            return Err(RetentionInventoryError::PolicyInvalid);
        }
        for table in &family.tables {
            if !valid_identifier(table)
                || seen_tables.insert(table.clone(), family.dataset_family.clone()).is_some()
            {
                return Err(RetentionInventoryError::PolicyInvalid);
            }
        }
    }
    if policy.required_operator_decisions.iter().any(|decision| !valid_identifier(decision)) {
        return Err(RetentionInventoryError::PolicyInvalid);
    }
    Ok(policy)
}

fn build_policy_table_inventory(
    policy: &RetentionPolicy,
    relations: &[PostgresRelationInventory],
) -> Vec<PostgresRetentionTableInventory> {
    let relation_by_name = relations
        .iter()
        .filter(|relation| relation.schema_name == "public")
        .map(|relation| (relation.relation_name.as_str(), relation))
        .collect::<BTreeMap<_, _>>();
    let mut tables = Vec::new();
    for family in &policy.families {
        for table_name in &family.tables {
            let relation = relation_by_name.get(table_name.as_str()).copied();
            tables.push(PostgresRetentionTableInventory {
                dataset_family: family.dataset_family.clone(),
                feature_owner: family.feature_owner.clone(),
                policy_state: family.policy_state.clone(),
                candidate_class: family.candidate_class.clone(),
                table_name: table_name.clone(),
                relation_present: relation.is_some(),
                estimated_rows: relation.map(|value| value.estimated_rows),
                estimated_dead_rows: relation.map(|value| value.estimated_dead_rows),
                catalog_rows_estimate: relation.map(|value| value.catalog_rows_estimate),
                last_analyze: relation.and_then(|value| value.last_analyze),
                last_autoanalyze: relation.and_then(|value| value.last_autoanalyze),
                table_bytes: relation.map(|value| value.table_bytes),
                index_bytes: relation.map(|value| value.index_bytes),
                total_bytes: relation.map(|value| value.total_bytes),
                eligibility_assessed: false,
                eligibility_authorized: false,
            });
        }
    }
    tables
}

fn build_policy_family_inventory(
    policy: &RetentionPolicy,
    tables: &[PostgresRetentionTableInventory],
) -> Vec<PostgresRetentionFamilyInventory> {
    policy
        .families
        .iter()
        .map(|family| {
            let family_tables =
                tables.iter().filter(|table| table.dataset_family == family.dataset_family);
            let mut configured_table_count = 0_i64;
            let mut present_table_count = 0_i64;
            let mut estimated_rows = 0_i64;
            let mut estimated_dead_rows = 0_i64;
            let mut total_bytes = 0_i64;
            let mut statistics_complete = true;
            for table in family_tables {
                configured_table_count += 1;
                if table.relation_present {
                    present_table_count += 1;
                } else {
                    statistics_complete = false;
                }
                let Some(rows) = table.estimated_rows else {
                    statistics_complete = false;
                    continue;
                };
                estimated_rows = estimated_rows.saturating_add(rows.max(0));
                let Some(dead_rows) = table.estimated_dead_rows else {
                    statistics_complete = false;
                    continue;
                };
                estimated_dead_rows = estimated_dead_rows.saturating_add(dead_rows.max(0));
                let Some(bytes) = table.total_bytes else {
                    statistics_complete = false;
                    continue;
                };
                total_bytes = total_bytes.saturating_add(bytes.max(0));
            }
            PostgresRetentionFamilyInventory {
                dataset_family: family.dataset_family.clone(),
                feature_owner: family.feature_owner.clone(),
                policy_state: family.policy_state.clone(),
                candidate_class: family.candidate_class.clone(),
                configured_table_count,
                present_table_count,
                estimated_rows,
                estimated_dead_rows,
                total_bytes,
                statistics_complete,
                eligibility_assessed: false,
                eligibility_authorized: false,
            }
        })
        .collect()
}

async fn read_realtime_inventory(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresRealtimeInventory, RetentionInventoryError> {
    let rows = sqlx::query(
        "WITH selected_streams AS (
             SELECT stream_key
             FROM public.realtime_streams
             ORDER BY stream_key ASC
             LIMIT $1
         ), event_summary AS (
             SELECT event.stream_key,
                    MIN(event.stream_sequence)::bigint AS min_sequence,
                    MAX(event.stream_sequence)::bigint AS max_sequence,
                    COUNT(*)::bigint AS event_count
             FROM public.realtime_events AS event
             JOIN selected_streams AS selected ON selected.stream_key = event.stream_key
             GROUP BY event.stream_key
         ), offset_summary AS (
             SELECT offset_row.stream_key,
                    MIN(offset_row.last_sequence)::bigint AS diagnostic_min_consumer_offset,
                    COUNT(*)::bigint AS consumer_count
             FROM public.realtime_consumer_offsets AS offset_row
             JOIN selected_streams AS selected ON selected.stream_key = offset_row.stream_key
             GROUP BY offset_row.stream_key
         ), failure_summary AS (
             SELECT event.stream_key,
                    MIN(event.stream_sequence) FILTER (WHERE failure.resolved_at IS NULL)::bigint
                        AS unresolved_failure_min_sequence,
                    MAX(event.stream_sequence) FILTER (WHERE failure.resolved_at IS NULL)::bigint
                        AS unresolved_failure_max_sequence
             FROM public.realtime_failures AS failure
             JOIN public.realtime_events AS event ON event.event_id = failure.event_id
             JOIN selected_streams AS selected ON selected.stream_key = event.stream_key
             GROUP BY event.stream_key
         )
         SELECT selected.stream_key,
                event.min_sequence,
                event.max_sequence,
                COALESCE(event.event_count, 0)::bigint AS event_count,
                COALESCE(offsets.consumer_count, 0)::bigint AS consumer_count,
                offsets.diagnostic_min_consumer_offset,
                COALESCE((
                    SELECT COUNT(*)::bigint
                    FROM public.realtime_events AS candidate
                    WHERE candidate.stream_key = selected.stream_key
                      AND offsets.diagnostic_min_consumer_offset IS NOT NULL
                      AND candidate.stream_sequence <= offsets.diagnostic_min_consumer_offset
                ), 0)::bigint AS events_below_diagnostic_min_offset,
                failures.unresolved_failure_min_sequence,
                failures.unresolved_failure_max_sequence
         FROM selected_streams AS selected
         LEFT JOIN event_summary AS event ON event.stream_key = selected.stream_key
         LEFT JOIN offset_summary AS offsets ON offsets.stream_key = selected.stream_key
         LEFT JOIN failure_summary AS failures ON failures.stream_key = selected.stream_key
         ORDER BY selected.stream_key ASC",
    )
    .bind(REALTIME_STREAM_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("realtime_stream_inventory", source))?;
    let streams = rows.into_iter().map(map_realtime_stream).collect::<Result<Vec<_>, _>>()?;

    let coverage_row = sqlx::query(
        "SELECT
             (SELECT COUNT(*)::bigint FROM public.realtime_streams) AS configured_stream_count,
             (SELECT COUNT(*)::bigint FROM public.realtime_consumer_offsets)
                 AS configured_consumer_offset_count",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("realtime_coverage", source))?;
    let configured_stream_count: i64 = coverage_row
        .try_get("configured_stream_count")
        .map_err(|source| query_error("realtime_coverage", source))?;
    let configured_consumer_offset_count: i64 = coverage_row
        .try_get("configured_consumer_offset_count")
        .map_err(|source| query_error("realtime_coverage", source))?;
    let diagnostic_consumer_offset_count = streams.iter().map(|stream| stream.consumer_count).sum();
    let reported_stream_count = i64::try_from(streams.len()).unwrap_or(MAX_REALTIME_STREAMS);

    Ok(PostgresRealtimeInventory {
        streams,
        coverage: PostgresRealtimeCoverage {
            configured_stream_count,
            reported_stream_count,
            stream_rows_truncated: configured_stream_count > MAX_REALTIME_STREAMS,
            configured_consumer_offset_count,
            diagnostic_consumer_offset_count,
            eligibility_assessed: false,
            eligibility_authorized: false,
        },
    })
}

fn map_realtime_stream(
    row: PgRow,
) -> Result<PostgresRealtimeStreamInventory, RetentionInventoryError> {
    Ok(PostgresRealtimeStreamInventory {
        stream_key: row
            .try_get("stream_key")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        min_sequence: row
            .try_get("min_sequence")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        max_sequence: row
            .try_get("max_sequence")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        event_count: row
            .try_get("event_count")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        consumer_count: row
            .try_get("consumer_count")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        diagnostic_min_consumer_offset: row
            .try_get("diagnostic_min_consumer_offset")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        events_below_diagnostic_min_offset: row
            .try_get("events_below_diagnostic_min_offset")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        unresolved_failure_min_sequence: row
            .try_get("unresolved_failure_min_sequence")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        unresolved_failure_max_sequence: row
            .try_get("unresolved_failure_max_sequence")
            .map_err(|source| query_error("realtime_stream_inventory", source))?,
        watermark_kind: "all_configured_consumers_not_authoritative_safe_watermark",
        eligibility_assessed: false,
        eligibility_authorized: false,
    })
}

async fn read_foreign_key_inventory(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresForeignKeyInventoryReport, RetentionInventoryError> {
    let rows = sqlx::query(
        "SELECT
             source.relname AS source_table,
             target.relname AS referenced_table,
             constraint_row.conname AS constraint_name,
             CASE constraint_row.confdeltype
                 WHEN 'a' THEN 'no_action'
                 WHEN 'r' THEN 'restrict'
                 WHEN 'c' THEN 'cascade'
                 WHEN 'n' THEN 'set_null'
                 WHEN 'd' THEN 'set_default'
                 ELSE 'unknown'
             END AS delete_action,
             COUNT(*) OVER ()::bigint AS foreign_key_count
         FROM pg_constraint AS constraint_row
         JOIN pg_class AS source ON source.oid = constraint_row.conrelid
         JOIN pg_namespace AS source_namespace ON source_namespace.oid = source.relnamespace
         JOIN pg_class AS target ON target.oid = constraint_row.confrelid
         JOIN pg_namespace AS target_namespace ON target_namespace.oid = target.relnamespace
         WHERE constraint_row.contype = 'f'
           AND source_namespace.nspname = 'public'
           AND target_namespace.nspname = 'public'
         ORDER BY target.relname ASC, source.relname ASC, constraint_row.conname ASC
         LIMIT $1",
    )
    .bind(FOREIGN_KEY_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("foreign_key_inventory", source))?;
    let foreign_key_count = if let Some(row) = rows.first() {
        row.try_get("foreign_key_count")
            .map_err(|source| query_error("foreign_key_inventory", source))?
    } else {
        0
    };
    let relations = rows
        .into_iter()
        .take(MAX_FOREIGN_KEYS as usize)
        .map(map_foreign_key)
        .collect::<Result<Vec<_>, _>>()?;
    let reported_foreign_key_count = i64::try_from(relations.len()).unwrap_or(MAX_FOREIGN_KEYS);
    Ok(PostgresForeignKeyInventoryReport {
        relations,
        coverage: PostgresForeignKeyCoverage {
            foreign_key_count,
            reported_foreign_key_count,
            foreign_key_rows_truncated: foreign_key_count > MAX_FOREIGN_KEYS,
            eligibility_assessed: false,
            eligibility_authorized: false,
        },
    })
}

fn map_foreign_key(row: PgRow) -> Result<PostgresForeignKeyInventory, RetentionInventoryError> {
    Ok(PostgresForeignKeyInventory {
        source_table: row
            .try_get("source_table")
            .map_err(|source| query_error("foreign_key_inventory", source))?,
        referenced_table: row
            .try_get("referenced_table")
            .map_err(|source| query_error("foreign_key_inventory", source))?,
        constraint_name: row
            .try_get("constraint_name")
            .map_err(|source| query_error("foreign_key_inventory", source))?,
        delete_action: row
            .try_get("delete_action")
            .map_err(|source| query_error("foreign_key_inventory", source))?,
        dependency_is_eligibility_blocker_until_policy_defined: true,
        eligibility_assessed: false,
        eligibility_authorized: false,
    })
}

async fn set_read_only_limits(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), RetentionInventoryError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut **transaction)
        .await
        .map_err(|source| query_error("transaction_policy", source))?;
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut **transaction)
        .await
        .map_err(|source| query_error("statement_timeout", source))?;
    sqlx::query("SET LOCAL lock_timeout = '1s'")
        .execute(&mut **transaction)
        .await
        .map_err(|source| query_error("lock_timeout", source))?;
    Ok(())
}

async fn read_schema_identity(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresSchemaIdentity, RetentionInventoryError> {
    let row = sqlx::query(
        "SELECT application, baseline_version, installed_at
         FROM public.auction_schema_identity
         WHERE singleton
         LIMIT 2",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|source| query_error("schema_identity", source))?;
    let Some(row) = row else {
        return Err(RetentionInventoryError::SchemaIdentityInvalid);
    };
    let application: String =
        row.try_get("application").map_err(|source| query_error("schema_identity", source))?;
    let baseline_version: i64 =
        row.try_get("baseline_version").map_err(|source| query_error("schema_identity", source))?;
    let installed_at: DateTime<Utc> =
        row.try_get("installed_at").map_err(|source| query_error("schema_identity", source))?;
    if application != "auction" || baseline_version <= 0 {
        return Err(RetentionInventoryError::SchemaIdentityInvalid);
    }
    Ok(PostgresSchemaIdentity { application, baseline_version, installed_at })
}

async fn read_migration_inventory(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresMigrationInventory, RetentionInventoryError> {
    let summary = sqlx::query(
        "SELECT COUNT(*)::bigint AS migration_count,
                COALESCE(MAX(version), 0)::bigint AS latest_version,
                COALESCE(BOOL_AND(success), true) AS all_success
         FROM public._sqlx_migrations",
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(|source| query_error("migration_summary", source))?;
    let latest_description = sqlx::query_scalar::<_, String>(
        "SELECT description
         FROM public._sqlx_migrations
         ORDER BY version DESC
         LIMIT 1",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|source| query_error("migration_description", source))?;
    let migration_count: i64 = summary
        .try_get("migration_count")
        .map_err(|source| query_error("migration_summary", source))?;
    let latest_version: i64 = summary
        .try_get("latest_version")
        .map_err(|source| query_error("migration_summary", source))?;
    let all_success: bool = summary
        .try_get("all_success")
        .map_err(|source| query_error("migration_summary", source))?;
    let expected_version = super::postgres_expected_migration_version();
    Ok(PostgresMigrationInventory {
        migration_count,
        latest_version,
        latest_description,
        all_success,
        expected_version,
        current: all_success && latest_version == expected_version,
    })
}

async fn read_relations(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<Vec<PostgresRelationInventory>, RetentionInventoryError> {
    let rows = sqlx::query(
        "SELECT namespace.nspname AS schema_name,
                relation.relname AS relation_name,
                relation.relkind::text AS relation_kind,
                COALESCE(stats.n_live_tup, 0)::bigint AS estimated_rows,
                COALESCE(stats.n_dead_tup, 0)::bigint AS estimated_dead_rows,
                relation.reltuples::bigint AS catalog_rows_estimate,
                stats.last_analyze AS last_analyze,
                stats.last_autoanalyze AS last_autoanalyze,
                pg_table_size(relation.oid)::bigint AS table_bytes,
                pg_indexes_size(relation.oid)::bigint AS index_bytes,
                pg_total_relation_size(relation.oid)::bigint AS total_bytes
         FROM pg_class AS relation
         JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
         LEFT JOIN pg_stat_user_tables AS stats ON stats.relid = relation.oid
         WHERE namespace.nspname = 'public'
           AND relation.relkind IN ('r', 'p', 'm', 'f')
         ORDER BY relation.relname ASC
         LIMIT $1",
    )
    .bind(RELATION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("relation_inventory", source))?;
    if rows.len() as i64 > MAX_RELATIONS {
        return Err(RetentionInventoryError::TooManyRelations(MAX_RELATIONS));
    }

    rows.into_iter().map(map_relation).collect()
}

fn map_relation(row: PgRow) -> Result<PostgresRelationInventory, RetentionInventoryError> {
    Ok(PostgresRelationInventory {
        schema_name: row
            .try_get("schema_name")
            .map_err(|source| query_error("relation_inventory", source))?,
        relation_name: row
            .try_get("relation_name")
            .map_err(|source| query_error("relation_inventory", source))?,
        relation_kind: row
            .try_get("relation_kind")
            .map_err(|source| query_error("relation_inventory", source))?,
        estimated_rows: row
            .try_get("estimated_rows")
            .map_err(|source| query_error("relation_inventory", source))?,
        estimated_dead_rows: row
            .try_get("estimated_dead_rows")
            .map_err(|source| query_error("relation_inventory", source))?,
        catalog_rows_estimate: row
            .try_get("catalog_rows_estimate")
            .map_err(|source| query_error("relation_inventory", source))?,
        last_analyze: row
            .try_get("last_analyze")
            .map_err(|source| query_error("relation_inventory", source))?,
        last_autoanalyze: row
            .try_get("last_autoanalyze")
            .map_err(|source| query_error("relation_inventory", source))?,
        table_bytes: row
            .try_get("table_bytes")
            .map_err(|source| query_error("relation_inventory", source))?,
        index_bytes: row
            .try_get("index_bytes")
            .map_err(|source| query_error("relation_inventory", source))?,
        total_bytes: row
            .try_get("total_bytes")
            .map_err(|source| query_error("relation_inventory", source))?,
    })
}

fn identity_string(
    row: &PgRow,
    column: &'static str,
    stage: &'static str,
) -> Result<String, RetentionInventoryError> {
    row.try_get(column).map_err(|source| query_error(stage, source))
}

fn query_error(stage: &'static str, source: sqlx::Error) -> RetentionInventoryError {
    RetentionInventoryError::Query { stage, source }
}

fn validate_database_name(value: &str) -> Result<(), RetentionInventoryError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > 63
        || value.chars().any(char::is_control)
    {
        return Err(RetentionInventoryError::InvalidDatabaseName);
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value).iter().map(|byte| format!("{byte:02x}")).collect()
}
