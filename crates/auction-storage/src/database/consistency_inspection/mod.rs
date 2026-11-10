//! 内部一致性巡检：有界只读快照与独立的只追加报告事实。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use super::DatabaseStore;

mod queries;
#[cfg(test)]
mod tests;

/// 每类最多保存的差异样本；总数仍由完整查询计算。
const SAMPLE_LIMIT: i64 = 100;
const STATEMENT_TIMEOUT_MS: u64 = 5_000;
const INSPECTION_TIMEOUT_SECONDS: u64 = 40;

/// 持久化与本地证据共用的巡检报告；不携带凭据、投递正文或资产动作授权。
#[derive(Debug, Serialize)]
pub struct ConsistencyInspectionReport {
    pub format: &'static str,
    pub report_id: Uuid,
    pub database_name: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub snapshot: Option<String>,
    pub completed_at: DateTime<Utc>,
    pub complete: bool,
    pub consistent: bool,
    pub sample_limit_per_check: i64,
    pub statement_timeout_ms: u64,
    pub inspection_timeout_seconds: u64,
    pub checks: Vec<ConsistencyCheckReport>,
    pub failure_code: Option<&'static str>,
    pub interrupted_invariant: Option<&'static str>,
    pub asset_actions_authorized: bool,
}

/// 单项不变量的全量差异计数与有界样本。
#[derive(Debug, Serialize)]
pub struct ConsistencyCheckReport {
    pub invariant: &'static str,
    pub violation_count: i64,
    pub samples_truncated: bool,
    pub samples: Vec<ConsistencyViolation>,
}

/// 差异只暴露数据库业务标识与固定原因码。
#[derive(Debug, Serialize)]
pub struct ConsistencyViolation {
    pub entity_key: String,
    pub reason: String,
}

/// 巡检拒绝或追加事实失败；数据库原始错误不跨边界，避免输出连接信息。
#[derive(Debug, thiserror::Error)]
pub enum ConsistencyInspectionError {
    #[error("巡检目标数据库名不匹配")]
    DatabaseMismatch,
    #[error("巡检数据库连接不可用")]
    DatabaseUnavailable,
    #[error("巡检报告追加保存失败")]
    ReportPersistence,
}

impl DatabaseStore {
    /// 在同一个 REPEATABLE READ READ ONLY 快照运行六类 SQL，不修复任何业务数据。
    /// 完成或中断均独立 INSERT 一条 audit_records，既有 append-only 触发器保护报告。
    pub async fn inspect_consistency(
        &self,
        database: &str,
    ) -> Result<ConsistencyInspectionReport, ConsistencyInspectionError> {
        let actual: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(self.pool())
            .await
            .map_err(database_unavailable)?;
        if database != actual {
            return Err(ConsistencyInspectionError::DatabaseMismatch);
        }
        let mut report = ConsistencyInspectionReport {
            format: "auction-consistency-inspection.v1",
            report_id: Uuid::new_v4(),
            database_name: actual,
            observed_at: None,
            snapshot: None,
            completed_at: Utc::now(),
            complete: false,
            consistent: false,
            sample_limit_per_check: SAMPLE_LIMIT,
            statement_timeout_ms: STATEMENT_TIMEOUT_MS,
            inspection_timeout_seconds: INSPECTION_TIMEOUT_SECONDS,
            checks: Vec::with_capacity(queries::CHECKS.len()),
            failure_code: None,
            interrupted_invariant: None,
            asset_actions_authorized: false,
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(INSPECTION_TIMEOUT_SECONDS),
            inspect_snapshot(self, &mut report),
        )
        .await
        .map_err(deadline_exceeded)
        .and_then(|result| result.map_err(query_failed));
        match result {
            Ok(()) => {
                report.complete = true;
                report.consistent = report.checks.iter().all(|check| check.violation_count == 0);
            }
            Err(failure_code) => report.failure_code = Some(failure_code),
        }
        report.completed_at = Utc::now();
        append_report(self, &report).await?;
        Ok(report)
    }
}

async fn inspect_snapshot(
    store: &DatabaseStore,
    report: &mut ConsistencyInspectionReport,
) -> Result<(), sqlx::Error> {
    let mut transaction = store.pool().begin_read_snapshot().await?;
    sqlx::query("SET LOCAL statement_timeout = '5s'").execute(&mut *transaction).await?;
    sqlx::query("SET LOCAL lock_timeout = '1s'").execute(&mut *transaction).await?;
    let identity = sqlx::query(
        "SELECT transaction_timestamp() AS observed_at, pg_current_snapshot()::text AS snapshot",
    )
    .fetch_one(&mut *transaction)
    .await?;
    report.observed_at = Some(identity.try_get("observed_at")?);
    report.snapshot = Some(identity.try_get("snapshot")?);
    for &(invariant, sql) in queries::CHECKS {
        report.interrupted_invariant = Some(invariant);
        // 子查询完整计数，窗口结果至多返回 100 个标识；客户端不载入业务行或正文。
        let rows = sqlx::query(sql).bind(SAMPLE_LIMIT).fetch_all(&mut *transaction).await?;
        let violation_count = rows
            .first()
            .map(|row| row.try_get::<i64, _>("violation_count"))
            .transpose()?
            .unwrap_or(0);
        let samples = rows
            .into_iter()
            .map(|row| {
                Ok(ConsistencyViolation {
                    entity_key: row.try_get("entity_key")?,
                    reason: row.try_get("reason")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        report.checks.push(ConsistencyCheckReport {
            invariant,
            violation_count,
            samples_truncated: violation_count > SAMPLE_LIMIT,
            samples,
        });
        report.interrupted_invariant = None;
    }
    transaction.commit().await
}

async fn append_report(
    store: &DatabaseStore,
    report: &ConsistencyInspectionReport,
) -> Result<(), ConsistencyInspectionError> {
    let metadata = serde_json::to_value(report).map_err(unserializable)?;
    let append = async {
        let mut transaction = store.pool().begin().await?;
        sqlx::query("SET LOCAL statement_timeout = '5s'").execute(&mut *transaction).await?;
        sqlx::query("SET LOCAL lock_timeout = '1s'").execute(&mut *transaction).await?;
        sqlx::query(
            "INSERT INTO public.audit_records
                 (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
             VALUES ($1, 'system', 'auction-cli', 'auction.consistency.inspect',
                     'consistency_report', $2, $3, 'system', $4)",
        )
        .bind(report.report_id)
        .bind(report.report_id.to_string())
        .bind(if report.consistent { "success" } else { "failure" })
        .bind(metadata)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), append)
        .await
        .map_err(append_timed_out)?
        .map_err(append_failed)
}

// 以下映射只把 SQLSTATE 等安全上下文写入日志；报告与返回值保持原有稳定分类。

fn database_unavailable(error: sqlx::Error) -> ConsistencyInspectionError {
    log_failure(&error, "inspection_database_unavailable");
    ConsistencyInspectionError::DatabaseUnavailable
}

fn query_failed(error: sqlx::Error) -> &'static str {
    log_failure(&error, "inspection_query_failed")
}

fn deadline_exceeded(_: tokio::time::error::Elapsed) -> &'static str {
    "inspection_deadline_exceeded"
}

fn append_failed(error: sqlx::Error) -> ConsistencyInspectionError {
    log_failure(&error, "inspection_report_append_failed");
    ConsistencyInspectionError::ReportPersistence
}

fn append_timed_out(_: tokio::time::error::Elapsed) -> ConsistencyInspectionError {
    ConsistencyInspectionError::ReportPersistence
}

/// serde 错误可能回显报告内容，只归类不记录。
fn unserializable(_: serde_json::Error) -> ConsistencyInspectionError {
    ConsistencyInspectionError::ReportPersistence
}

fn log_failure(error: &sqlx::Error, code: &'static str) -> &'static str {
    let context = super::sql_error_context(error, "一致性巡检失败");
    tracing::warn!(kind = code, phase = "consistency_inspection", context, "一致性巡检未完成");
    code
}
