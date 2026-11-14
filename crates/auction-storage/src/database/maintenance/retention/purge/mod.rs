//! 保留清理的有界执行：只接受一次新鲜只读演练回执派生的逐数据族批准，按政策 v2 的闭集
//! 谓词分批删除运行态行与加密诊断文件。审计、资金、业务事实、有效/永久会话、未解决失败
//! 与写入未知诊断永不在删除范围；每批一个事务，失败时已提交批次保留并在回执中精确计数。

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::TrackedPool;
use crate::database::maintenance::retention::inventory::{
    load_retention_policy_for_dry_run, retention_policy_sha256,
};

mod model;
mod tables;
mod validation;

use self::tables::{RemainingCandidates, count_remaining, delete_statement};
pub use model::*;
use validation::validate_request;

const STATEMENT_TIMEOUT: &str = "30s";
const LOCK_TIMEOUT: &str = "2s";
const AUDIT_STATEMENT_TIMEOUT: &str = "10s";
/// 演练回执时间允许的时钟偏移。
const CLOCK_SKEW_SECONDS: i64 = 120;

struct ValidatedTable {
    table: &'static str,
    rule_ids: Vec<String>,
    candidate_rows: i64,
    cutoff: DateTime<Utc>,
}

struct ValidatedFamily {
    dataset_family: String,
    retention_kind: String,
    candidate_rows: i64,
    candidate_bytes: Option<u64>,
    tables: Vec<ValidatedTable>,
}

impl DatabaseStore {
    /// 在任何变更前完成全部预校验：政策/令牌/计数/新鲜度与数据库身份、迁移前缀。
    /// 文件族批准要求 `request.filesystem` 存在，因此调用方可先传零值占位结果完成预检，
    /// 执行实际文件清理后再以真实结果执行。
    pub async fn preflight_retention_purge(
        &self,
        request: &RetentionPurgeRequest,
    ) -> Result<(), RetentionPurgeError> {
        let policy = load_retention_policy_for_dry_run()
            .map_err(|_: crate::RetentionInventoryError| RetentionPurgeError::PolicyInvalid)?;
        let _ = validate_request(request, &policy)?;
        verify_target(self, &request.database_name).await
    }

    /// 执行一次已批准的保留清理。预校验失败不产生任何变更并返回错误；一旦开始执行，
    /// 无论完成、部分完成还是中途失败都返回回执，并由审计事实固定结果。
    pub async fn execute_retention_purge(
        &self,
        request: RetentionPurgeRequest,
    ) -> Result<RetentionPurgeReport, RetentionPurgeError> {
        let policy = load_retention_policy_for_dry_run()
            .map_err(|_: crate::RetentionInventoryError| RetentionPurgeError::PolicyInvalid)?;
        let validated = validate_request(&request, &policy)?;
        verify_target(self, &request.database_name).await?;
        // 先清理已解决的推送失败，再清理事件：删除失败记录可能解除事件的 RESTRICT 引用，
        // 使同一批批准上限内的事件候选取值单调收敛；反向顺序会留下只能靠下一轮演练清理的行。
        let mut order = (0..validated.len()).collect::<Vec<_>>();
        order.sort_by_key(|index| execution_rank(&validated[*index].dataset_family));

        let run_id = Uuid::new_v4();
        let started_at = Utc::now();
        // 文件族可能已由调用方在预校验之后删除；即使后续数据库批次失败，也必须如实计入。
        let mut mutation_executed =
            request.filesystem.as_ref().is_some_and(|filesystem| filesystem.deleted_files > 0);
        let mut failure_code: Option<&'static str> = None;
        let mut families = Vec::with_capacity(validated.len());
        let mut skipped = Vec::new();

        for (position, index) in order.iter().enumerate() {
            if failure_code.is_some() {
                skipped.extend(
                    order[position..].iter().map(|index| validated[*index].dataset_family.clone()),
                );
                break;
            }
            let family = &validated[*index];
            if family.retention_kind == "filesystem" {
                // 文件族已在调用方按批准上限执行；这里只把实际 unlink 计数并入回执与审计。
                if let Some(result) = &request.filesystem {
                    mutation_executed = mutation_executed || result.deleted_files > 0;
                    if result.failure_code.is_some() {
                        failure_code = Some("filesystem_purge_failed");
                    }
                    let outcome = if result.failure_code.is_some() {
                        RetentionPurgeOutcome::Failed
                    } else if result.limit_reached {
                        RetentionPurgeOutcome::PartialLimitReached
                    } else {
                        RetentionPurgeOutcome::Complete
                    };
                    families.push(RetentionPurgeFamilyResult {
                        dataset_family: family.dataset_family.clone(),
                        retention_kind: family.retention_kind.clone(),
                        approved_rows: family.candidate_rows,
                        approved_bytes: family.candidate_bytes,
                        deleted_rows: i64::try_from(result.deleted_files).map_err(
                            |_: std::num::TryFromIntError| RetentionPurgeError::CountOverflow,
                        )?,
                        deleted_bytes: i64::try_from(result.deleted_bytes).map_err(
                            |_: std::num::TryFromIntError| RetentionPurgeError::CountOverflow,
                        )?,
                        tables: Vec::new(),
                        outcome,
                    });
                }
                continue;
            }

            let mut table_results = Vec::with_capacity(family.tables.len());
            let mut family_outcome = RetentionPurgeOutcome::Complete;
            for table in &family.tables {
                let Some(statement) = delete_statement(table.table) else {
                    failure_code = Some("table_not_purgeable");
                    break;
                };
                let mut deleted_rows = 0_i64;
                let mut deleted_row_bytes = 0_i64;
                let mut batches = 0_u32;
                let mut table_failure = None;
                let mut remaining_limit = table.candidate_rows;
                while remaining_limit > 0 {
                    let limit = remaining_limit.min(i64::from(request.batch_size));
                    match delete_batch(self, statement, table.cutoff, limit).await {
                        Ok((rows, bytes)) => {
                            if rows == 0 {
                                break;
                            }
                            mutation_executed = true;
                            batches = batches.saturating_add(1);
                            deleted_rows = add(deleted_rows, rows)?;
                            deleted_row_bytes = add(deleted_row_bytes, bytes)?;
                            remaining_limit -= rows;
                        }
                        Err(error) => {
                            table_failure = Some(error);
                            break;
                        }
                    }
                }
                let remaining = match count_remaining(self.pool(), table.table, table.cutoff).await
                {
                    Ok(counts) => counts,
                    Err(error) => {
                        table_failure = Some(error);
                        RemainingCandidates { remaining: 0, blocked: 0 }
                    }
                };
                if remaining.remaining > 0 {
                    family_outcome = RetentionPurgeOutcome::PartialLimitReached;
                }
                table_results.push(RetentionPurgeTableResult {
                    table: table.table.to_owned(),
                    rule_ids: table.rule_ids.clone(),
                    approved_rows: table.candidate_rows,
                    deleted_rows,
                    deleted_row_bytes,
                    batches,
                    remaining_candidate_rows: remaining.remaining,
                    blocked_candidate_rows: remaining.blocked,
                });
                if let Some(error) = table_failure {
                    failure_code = Some(purge_failure_code(&error));
                    break;
                }
            }
            if failure_code.is_some() {
                family_outcome = RetentionPurgeOutcome::Failed;
            }
            families.push(RetentionPurgeFamilyResult {
                dataset_family: family.dataset_family.clone(),
                retention_kind: family.retention_kind.clone(),
                approved_rows: family.candidate_rows,
                approved_bytes: family.candidate_bytes,
                deleted_rows: table_results.iter().map(|table| table.deleted_rows).sum(),
                deleted_bytes: table_results.iter().map(|table| table.deleted_row_bytes).sum(),
                tables: table_results,
                outcome: family_outcome,
            });
        }

        // 顶层计数只汇总数据库行；文件族的文件数/字节只在 `filesystem` 与族结果中报告。
        let deleted_rows = families
            .iter()
            .filter(|family| family.retention_kind == "database_rows")
            .map(|family| family.deleted_rows)
            .sum();
        let deleted_row_bytes = families
            .iter()
            .filter(|family| family.retention_kind == "database_rows")
            .map(|family| family.deleted_bytes)
            .sum();
        let outcome = if failure_code.is_some() {
            RetentionPurgeOutcome::Failed
        } else if families
            .iter()
            .any(|family| family.outcome == RetentionPurgeOutcome::PartialLimitReached)
        {
            RetentionPurgeOutcome::PartialLimitReached
        } else {
            RetentionPurgeOutcome::Complete
        };
        let mut notes = vec![
            "只删除政策 v2 运行态数据族的候选；审计、资金、业务历史、身份与配置事实永不删除"
                .to_owned(),
            "有效会话（含永久会话）、未解决失败与写入未知诊断永不在删除范围".to_owned(),
            "候选边界为演练快照时间减期限；已注销会话立即候选".to_owned(),
            "每批一个事务；失败时已提交批次保留，回执与审计记录精确计数".to_owned(),
            "执行顺序先清理已解决的推送失败、再清理事件，使同轮解除引用的事件也可在批准上限内删除"
                .to_owned(),
            "deleted_row_bytes 为删除时 pg_column_size 的行载荷字节数，不是物理页面大小".to_owned(),
        ];
        if !skipped.is_empty() {
            notes.push(format!("失败后跳过的数据族：{}", skipped.join(", ")));
        }

        let mut report = RetentionPurgeReport {
            format: RETENTION_PURGE_RECEIPT_FORMAT,
            run_id,
            dry_run_run_id: request.dry_run_run_id,
            database_name: request.database_name.clone(),
            policy_version: policy.version,
            policy_sha256: retention_policy_sha256(),
            batch_size: request.batch_size,
            started_at,
            finished_at: Utc::now(),
            outcome,
            failure_code,
            families,
            deleted_rows,
            deleted_row_bytes,
            filesystem: request.filesystem.clone(),
            audit_id: None,
            mutation_executed,
            deletion_authorized: true,
            notes,
        };
        if let Ok(audit_id) = append_audit(self.pool(), &report).await {
            report.audit_id = Some(audit_id);
        } else {
            report.outcome = RetentionPurgeOutcome::Failed;
            report.failure_code = Some("audit_append_failed");
        }
        report.finished_at = Utc::now();
        Ok(report)
    }
}

/// 执行次序：先清理可能解除事件外键阻断的失败记录，再清理事件，其他族保持政策顺序。
fn execution_rank(dataset_family: &str) -> u8 {
    match dataset_family {
        "realtime_failures" => 0,
        "realtime_events" => 2,
        _ => 1,
    }
}

fn purge_failure_code(error: &RetentionPurgeError) -> &'static str {
    match error {
        RetentionPurgeError::Query { stage, .. } => match *stage {
            "begin_batch" => "batch_begin_failed",
            "delete_batch" => "delete_batch_failed",
            "count_remaining" => "remaining_count_failed",
            _ => "query_failed",
        },
        RetentionPurgeError::Commit { .. } => "batch_commit_failed",
        RetentionPurgeError::CountOverflow => "count_overflow",
        _ => "purge_failed",
    }
}

async fn verify_target(
    store: &DatabaseStore,
    database_name: &str,
) -> Result<(), RetentionPurgeError> {
    let actual: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(store.pool())
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "database_identity", source })?;
    if actual != database_name {
        return Err(RetentionPurgeError::DatabaseMismatch {
            expected: database_name.to_owned(),
            actual,
        });
    }
    require_current_migrations(store.pool()).await
}

async fn require_current_migrations(
    pool: &TrackedPool<sqlx::Postgres>,
) -> Result<(), RetentionPurgeError> {
    let expected_version = crate::database::postgres_expected_migration_version();
    let marker_exists =
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await
            .map_err(|source| RetentionPurgeError::Query { stage: "migration_marker", source })?;
    if !marker_exists {
        return Err(RetentionPurgeError::MigrationNotCurrent);
    }
    let migrations = sqlx::query(
        "SELECT version, description, success, checksum
         FROM public._sqlx_migrations
         ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .map_err(|source| RetentionPurgeError::Query { stage: "migration_identity", source })?;
    if !crate::database::migrations::postgres_history_matches(&migrations, expected_version) {
        return Err(RetentionPurgeError::MigrationNotCurrent);
    }
    Ok(())
}

async fn delete_batch(
    store: &DatabaseStore,
    statement: &'static str,
    cutoff: DateTime<Utc>,
    limit: i64,
) -> Result<(i64, i64), RetentionPurgeError> {
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "begin_batch", source })?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = '{STATEMENT_TIMEOUT}'"
    )))
    .execute(&mut *transaction)
    .await
    .map_err(|source| RetentionPurgeError::Query { stage: "statement_timeout", source })?;
    sqlx::query(sqlx::AssertSqlSafe(format!("SET LOCAL lock_timeout = '{LOCK_TIMEOUT}'")))
        .execute(&mut *transaction)
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "lock_timeout", source })?;
    let row = sqlx::query(sqlx::AssertSqlSafe(statement))
        .bind(cutoff)
        .bind(limit)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "delete_batch", source })?;
    let rows: i64 = row
        .try_get("deleted_rows")
        .map_err(|source| RetentionPurgeError::Query { stage: "delete_batch", source })?;
    let bytes: i64 = row
        .try_get("deleted_bytes")
        .map_err(|source| RetentionPurgeError::Query { stage: "delete_batch", source })?;
    transaction
        .commit()
        .await
        .map_err(|source| RetentionPurgeError::Commit { stage: "commit_batch", source })?;
    Ok((rows, bytes))
}

async fn append_audit(
    pool: &TrackedPool<sqlx::Postgres>,
    report: &RetentionPurgeReport,
) -> Result<Uuid, RetentionPurgeError> {
    let id = Uuid::new_v4();
    let families = report
        .families
        .iter()
        .map(|family| {
            serde_json::json!({
                "dataset_family": family.dataset_family,
                "retention_kind": family.retention_kind,
                "approved_rows": family.approved_rows,
                "approved_bytes": family.approved_bytes,
                "deleted_rows": family.deleted_rows,
                "deleted_bytes": family.deleted_bytes,
                "outcome": family.outcome,
                "tables": family.tables,
            })
        })
        .collect::<Vec<_>>();
    let metadata = serde_json::json!({
        "database_name": report.database_name,
        "dry_run_run_id": report.dry_run_run_id,
        "purge_run_id": report.run_id,
        "policy_version": report.policy_version,
        "policy_sha256": report.policy_sha256,
        "batch_size": report.batch_size,
        "outcome": report.outcome,
        "failure_code": report.failure_code,
        "families": families,
        "deleted_rows": report.deleted_rows,
        "deleted_row_bytes": report.deleted_row_bytes,
        "filesystem": report.filesystem,
        "mutation_executed": report.mutation_executed,
    });
    let mut transaction = pool
        .begin()
        .await
        .map_err(|source| RetentionPurgeError::Query { stage: "audit_begin", source })?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL statement_timeout = '{AUDIT_STATEMENT_TIMEOUT}'"
    )))
    .execute(&mut *transaction)
    .await
    .map_err(|source| RetentionPurgeError::Query { stage: "audit_timeout", source })?;
    sqlx::query(
        "INSERT INTO public.audit_records
             (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, 'system', 'auction-cli', 'auction.retention.purge',
                 'retention_purge', $2, $3, 'system', $4)",
    )
    .bind(id)
    .bind(report.run_id.to_string())
    .bind(if report.outcome == RetentionPurgeOutcome::Complete { "success" } else { "failure" })
    .bind(metadata)
    .execute(&mut *transaction)
    .await
    .map_err(|source| RetentionPurgeError::Query { stage: "audit_insert", source })?;
    transaction
        .commit()
        .await
        .map_err(|source| RetentionPurgeError::Commit { stage: "audit_commit", source })?;
    Ok(id)
}

fn add(left: i64, right: i64) -> Result<i64, RetentionPurgeError> {
    left.checked_add(right).ok_or(RetentionPurgeError::CountOverflow)
}

#[cfg(test)]
mod tests {
    use super::tables::{PURGEABLE_TABLES, delete_statement, remaining_statement};
    use super::validation::{purgeable_table, retention_cutoff};
    use chrono::{Days, TimeZone, Utc};

    #[test]
    fn allowlist_is_exactly_the_runtime_families() {
        assert_eq!(
            PURGEABLE_TABLES,
            [
                "service_heartbeats",
                "realtime_events",
                "realtime_failures",
                "public_sessions",
                "admin_sessions",
            ]
        );
        for protected in [
            "audit_records",
            "bids",
            "compensation_ledger",
            "lot_settlements",
            "users",
            "user_login_key_envelopes",
            "deliveries",
            "inbound_mail",
        ] {
            assert!(purgeable_table(protected).is_none(), "{protected}");
            assert!(delete_statement(protected).is_none(), "{protected}");
            assert!(remaining_statement(protected).is_none(), "{protected}");
        }
    }

    #[test]
    fn every_purgeable_table_has_guarded_delete_and_count() {
        for table in PURGEABLE_TABLES {
            let delete = delete_statement(table).expect("delete statement");
            assert!(delete.contains("pg_column_size(t)"), "{table}");
            assert!(delete.contains("LIMIT $2"), "{table}");
            assert!(remaining_statement(table).is_some(), "{table}");
        }
        let events = delete_statement("realtime_events").expect("events");
        assert!(events.contains("realtime_failures"));
        assert!(events.contains("replay_of_event_id"));
        let failures = delete_statement("realtime_failures").expect("failures");
        assert!(failures.contains("resolved_at IS NOT NULL"));
        for sessions in ["public_sessions", "admin_sessions"] {
            let statement = delete_statement(sessions).expect("sessions");
            assert!(statement.contains("revoked_at IS NOT NULL"), "{sessions}");
            assert!(statement.contains("expires_at IS NOT NULL"), "{sessions}");
        }
    }

    #[test]
    fn cutoff_is_snapshot_minus_policy_duration() {
        let snapshot = Utc.with_ymd_and_hms(2026, 11, 5, 12, 0, 0).single().expect("timestamp");
        assert_eq!(retention_cutoff(snapshot, 30), Some(snapshot - Days::new(30)));
        assert_eq!(retention_cutoff(snapshot, 0), Some(snapshot));
        assert_eq!(retention_cutoff(snapshot, -1), None);
    }
}
