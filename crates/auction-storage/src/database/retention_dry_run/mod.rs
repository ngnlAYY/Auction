//! 运行态数据族的保留清理只读演练：在同一 `REPEATABLE READ` 只读事务中按政策 v2 的期限
//! 统计候选行数，输出带 run id 的回执所需报告。该模块不执行 DELETE/UPDATE，也不读取行内容；
//! 候选数不是删除授权，实际清理须经用户批准后另行实现。

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;
use super::retention_inventory::{
    RetentionPolicy, RetentionPolicyFamily, load_retention_policy_for_dry_run,
    retention_policy_sha256, retention_rule_spec,
};

mod counts;
mod limits;
mod model;

use counts::*;
use limits::*;
pub use model::*;

impl DatabaseStore {
    /// 只读演练。`confirmed_database_name` 必须与连接实际返回的 `current_database()` 一致；
    /// 迁移历史必须是当前编译制品的完整干净前缀；任何条件不满足都在计数前失败。
    pub async fn collect_retention_dry_run(
        &self,
        confirmed_database_name: &str,
        run_id: Uuid,
    ) -> Result<RetentionDryRunReport, RetentionDryRunError> {
        if run_id.is_nil() {
            return Err(RetentionDryRunError::InvalidRunId);
        }
        if !crate::database::valid_confirmed_database_name(confirmed_database_name) {
            return Err(RetentionDryRunError::InvalidDatabaseName);
        }
        let policy = load_retention_policy_for_dry_run().map_err(|error| {
            let _ = error;
            RetentionDryRunError::PolicyInvalid
        })?;
        let expected_version = super::postgres_expected_migration_version();
        if policy.database_schema_version != expected_version {
            return Err(RetentionDryRunError::PolicyInvalid);
        }

        let mut transaction = self
            .pool()
            .begin_read_snapshot()
            .await
            .map_err(|source| query_error("begin_read_only_transaction", source))?;
        super::set_read_only_limits(&mut transaction)
            .await
            .map_err(|(stage, source)| RetentionDryRunError::Query { stage, source })?;
        // 候选计数需要扫描运行态表，比盘点估计值更重；仍保持有界只读，超时即失败关闭。
        sqlx::query("SET LOCAL statement_timeout = '30s'")
            .execute(&mut *transaction)
            .await
            .map_err(|source| query_error("statement_timeout", source))?;

        let identity = sqlx::query(
            "SELECT current_database() AS database_name,
                    current_setting('server_version') AS server_version,
                    transaction_timestamp() AS collected_at,
                    txid_current_snapshot()::text AS transaction_snapshot,
                    current_setting('transaction_read_only') AS transaction_read_only",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| query_error("database_identity", source))?;
        let database_name: String = identity
            .try_get("database_name")
            .map_err(|source| query_error("database_identity", source))?;
        if database_name != confirmed_database_name {
            return Err(RetentionDryRunError::DatabaseMismatch {
                expected: confirmed_database_name.to_owned(),
                actual: database_name,
            });
        }
        let server_version: String = identity
            .try_get("server_version")
            .map_err(|source| query_error("database_identity", source))?;
        let transaction_snapshot: String = identity
            .try_get("transaction_snapshot")
            .map_err(|source| query_error("database_identity", source))?;
        let transaction_read_only: String = identity
            .try_get("transaction_read_only")
            .map_err(|source| query_error("database_identity", source))?;
        if transaction_read_only != "on" {
            return Err(RetentionDryRunError::ReadOnlyTransactionInvalid);
        }
        let collected_at: DateTime<Utc> = identity
            .try_get("collected_at")
            .map_err(|source| query_error("database_identity", source))?;

        let marker_exists = sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| query_error("migration_marker", source))?;
        if !marker_exists {
            return Err(RetentionDryRunError::MigrationHistoryMissing);
        }
        let migrations = sqlx::query(
            "SELECT version, description, success, checksum
             FROM public._sqlx_migrations
             ORDER BY version",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(|source| query_error("migration_identity", source))?;
        if !super::migrations::postgres_history_matches(&migrations, expected_version) {
            return Err(RetentionDryRunError::MigrationNotCurrent);
        }

        let families = collect_families(&mut transaction, &policy, collected_at).await?;
        let mut candidate_rows = 0_i64;
        let mut retained_rows = 0_i64;
        for family in &families {
            candidate_rows = add(candidate_rows, family.candidate_rows)?;
            retained_rows = add(retained_rows, family.retained_rows)?;
        }

        transaction.commit().await.map_err(|source| RetentionDryRunError::Commit { source })?;

        Ok(RetentionDryRunReport {
            format: RETENTION_DRY_RUN_FORMAT,
            run_id,
            collected_at,
            database_name,
            server_version,
            transaction_snapshot,
            transaction_read_only,
            policy_id: policy.policy_id.clone(),
            policy_version: policy.version,
            policy_sha256: retention_policy_sha256(),
            deletion_requires_dry_run_and_operator_approval: policy
                .deletion_requires_dry_run_and_operator_approval,
            families,
            candidate_rows,
            retained_rows,
            mutation_executed: false,
            deletion_authorized: false,
            notes: vec![
                "本演练只在 REPEATABLE READ 只读事务中执行 SELECT，不执行 DELETE/UPDATE".to_owned(),
                "候选数不是删除授权；实际清理须另行批准并按表分批执行".to_owned(),
                "blocked_rows 为仍被外键引用、不能单独删除的候选事件行".to_owned(),
                "会话候选只含已注销或截止时间已过期满 30 天的记录；有效与永久会话被排除".to_owned(),
                "边界口径为时间戳 <= 快照时间减期限；恰好等于边界视为候选".to_owned(),
                "文件族（加密邮件诊断）不在数据库内，由 CLI 回执单独统计".to_owned(),
            ],
        })
    }
}

async fn collect_families(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    policy: &RetentionPolicy,
    now: DateTime<Utc>,
) -> Result<Vec<RetentionFamilyDryRun>, RetentionDryRunError> {
    let mut families = Vec::with_capacity(policy.families.len());
    for family in &policy.families {
        let Some(retention) = &family.retention else {
            continue;
        };
        if retention.kind != "database_rows" {
            continue;
        }
        let rules = family_rules(family, now)?;
        let cutoffs = rules
            .iter()
            .map(|rule| (rule.rule_id.clone(), rule.cutoff))
            .collect::<BTreeMap<_, _>>();
        let tables = count_family_tables(transaction, family, &cutoffs, now).await?;
        let mut candidate_rows = 0_i64;
        let mut retained_rows = 0_i64;
        let mut blocked_rows = 0_i64;
        let mut excluded_active_rows = 0_i64;
        for table in &tables {
            candidate_rows = add(candidate_rows, table.candidate_rows)?;
            retained_rows = add(retained_rows, table.retained_rows)?;
            blocked_rows = add(blocked_rows, table.blocked_rows)?;
            excluded_active_rows = add(excluded_active_rows, table.excluded_active_rows)?;
        }
        families.push(RetentionFamilyDryRun {
            dataset_family: family.dataset_family.clone(),
            feature_owner: family.feature_owner.clone(),
            policy_state: family.policy_state.clone(),
            retention_kind: retention.kind.clone(),
            rules,
            tables,
            candidate_rows,
            retained_rows,
            blocked_rows,
            excluded_active_rows,
        });
    }
    Ok(families)
}

fn family_rules(
    family: &RetentionPolicyFamily,
    now: DateTime<Utc>,
) -> Result<Vec<RetentionRuleDryRun>, RetentionDryRunError> {
    let retention = family.retention.as_ref().ok_or(RetentionDryRunError::PolicyInvalid)?;
    retention
        .rules
        .iter()
        .map(|rule| {
            let cutoff = retention_cutoff(now, rule.duration_days)
                .ok_or(RetentionDryRunError::PolicyInvalid)?;
            Ok(RetentionRuleDryRun {
                rule_id: rule.rule_id.clone(),
                table: rule.table.clone(),
                duration_days: rule.duration_days,
                cutoff,
            })
        })
        .collect()
}

async fn count_family_tables(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    family: &RetentionPolicyFamily,
    cutoffs: &BTreeMap<String, DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Vec<RetentionTableDryRun>, RetentionDryRunError> {
    let retention = family.retention.as_ref().ok_or(RetentionDryRunError::PolicyInvalid)?;
    let mut seen = BTreeSet::new();
    let mut tables = Vec::new();
    for rule in &retention.rules {
        let spec = retention_rule_spec(&rule.rule_id).ok_or(RetentionDryRunError::PolicyInvalid)?;
        let Some(table) = spec.table else {
            continue;
        };
        if !seen.insert(table) {
            continue;
        }
        let rule_ids = retention
            .rules
            .iter()
            .filter(|candidate| {
                retention_rule_spec(&candidate.rule_id).and_then(|spec| spec.table) == Some(table)
            })
            .map(|candidate| candidate.rule_id.clone())
            .collect::<Vec<_>>();
        tables.push(match table {
            "service_heartbeats" => {
                let counts =
                    heartbeats(transaction, cutoff(cutoffs, "service_heartbeat_expired_grace")?)
                        .await?;
                RetentionTableDryRun {
                    table: table.to_owned(),
                    rule_ids,
                    total_rows: counts.total,
                    candidate_rows: counts.candidates,
                    retained_rows: sub(counts.total, counts.candidates)?,
                    blocked_rows: 0,
                    excluded_active_rows: 0,
                    grace_rows: 0,
                    unresolved_rows: 0,
                }
            }
            "realtime_events" => {
                let counts =
                    realtime_events(transaction, cutoff(cutoffs, "realtime_event_age")?).await?;
                RetentionTableDryRun {
                    table: table.to_owned(),
                    rule_ids,
                    total_rows: counts.total,
                    candidate_rows: counts.candidates,
                    retained_rows: sub(counts.total, counts.candidates)?,
                    blocked_rows: counts.blocked,
                    excluded_active_rows: 0,
                    grace_rows: 0,
                    unresolved_rows: 0,
                }
            }
            "realtime_failures" => {
                let counts = realtime_failures(
                    transaction,
                    cutoff(cutoffs, "realtime_resolved_failure_age")?,
                )
                .await?;
                RetentionTableDryRun {
                    table: table.to_owned(),
                    rule_ids,
                    total_rows: counts.total,
                    candidate_rows: counts.candidates,
                    retained_rows: sub(counts.total, counts.candidates)?,
                    blocked_rows: 0,
                    excluded_active_rows: 0,
                    grace_rows: 0,
                    unresolved_rows: counts.unresolved,
                }
            }
            "public_sessions" => {
                let counts = sessions(
                    transaction,
                    "public_sessions",
                    cutoff(cutoffs, "public_session_expired_grace")?,
                    now,
                )
                .await?;
                session_table_counts("public_sessions", counts, rule_ids)?
            }
            "admin_sessions" => {
                let counts = sessions(
                    transaction,
                    "admin_sessions",
                    cutoff(cutoffs, "admin_session_expired_grace")?,
                    now,
                )
                .await?;
                session_table_counts("admin_sessions", counts, rule_ids)?
            }
            _ => return Err(RetentionDryRunError::PolicyInvalid),
        });
    }
    Ok(tables)
}

fn cutoff(
    cutoffs: &BTreeMap<String, DateTime<Utc>>,
    rule_id: &str,
) -> Result<DateTime<Utc>, RetentionDryRunError> {
    cutoffs.get(rule_id).copied().ok_or(RetentionDryRunError::PolicyInvalid)
}

pub(super) fn query_error(stage: &'static str, source: sqlx::Error) -> RetentionDryRunError {
    RetentionDryRunError::Query { stage, source }
}

#[cfg(test)]
mod tests {
    use super::{family_rules, load_retention_policy_for_dry_run};
    use chrono::{DateTime, Days};

    #[test]
    fn policy_v2_declares_runtime_families_with_decided_durations() {
        let policy = load_retention_policy_for_dry_run().expect("policy v2");
        assert_eq!(policy.version, 2);
        assert_eq!(policy.format, "auction-retention-policy.v2");
        assert!(policy.deletion_requires_dry_run_and_operator_approval);
        assert!(policy.audit_financial_and_business_facts_retain_forever);
        assert!(!policy.inventory_authorizes_mutation);

        let expected = [
            ("service_heartbeat_expired_grace", 30),
            ("realtime_event_age", 30),
            ("realtime_resolved_failure_age", 90),
            ("public_session_revoked", 0),
            ("public_session_expired_grace", 30),
            ("admin_session_revoked", 0),
            ("admin_session_expired_grace", 30),
            ("mail_diagnostic_ordinary_age", 7),
            ("mail_diagnostic_write_unknown_hold", 90),
        ];
        for (rule_id, duration_days) in expected {
            let family = policy
                .families
                .iter()
                .find(|family| {
                    family.retention.as_ref().is_some_and(|retention| {
                        retention.rules.iter().any(|rule| rule.rule_id == rule_id)
                    })
                })
                .expect("family with retention rule");
            let retention = family.retention.as_ref().expect("retention");
            let rule = retention.rules.iter().find(|rule| rule.rule_id == rule_id).expect("rule");
            assert_eq!(rule.duration_days, duration_days, "{rule_id}");
        }
    }

    #[test]
    fn retain_forever_families_never_carry_retention_rules() {
        let policy = load_retention_policy_for_dry_run().expect("policy v2");
        for family in &policy.families {
            if family.policy_state == "retain_forever" {
                assert!(family.retention.is_none(), "{}", family.dataset_family);
            } else {
                assert!(family.retention.is_some(), "{}", family.dataset_family);
            }
        }
    }

    #[test]
    fn audit_financial_and_business_families_are_retain_forever() {
        let policy = load_retention_policy_for_dry_run().expect("policy v2");
        for name in [
            "audit",
            "settlement_compensation_recovery",
            "auction_history",
            "identity_admin_safety",
        ] {
            let family = policy
                .families
                .iter()
                .find(|family| family.dataset_family == name)
                .expect("protected family");
            assert_eq!(family.policy_state, "retain_forever", "{name}");
            assert!(family.retention.is_none(), "{name}");
        }
    }

    #[test]
    fn family_rules_use_snapshot_minus_policy_duration() {
        let policy = load_retention_policy_for_dry_run().expect("policy v2");
        let family = policy
            .families
            .iter()
            .find(|family| family.dataset_family == "service_heartbeat")
            .expect("heartbeat family");
        let now = DateTime::from_timestamp(1_000_000_000, 0).expect("timestamp");
        let rules = family_rules(family, now).expect("rules");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].cutoff, now - Days::new(30));
    }
}
