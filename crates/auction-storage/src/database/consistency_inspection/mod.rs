//! W11.4 内部一致性巡检：有界只读快照与独立的只追加报告事实。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use super::PostgresStore;

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

impl PostgresStore {
    /// 在同一个 REPEATABLE READ READ ONLY 快照运行六类 SQL，不修复任何业务数据。
    /// 完成或中断均独立 INSERT 一条 audit_records，既有 append-only 触发器保护报告。
    pub async fn inspect_consistency(
        &self,
        database: &str,
    ) -> Result<ConsistencyInspectionReport, ConsistencyInspectionError> {
        let actual: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(self.pool())
            .await
            .map_err(|error| {
                let _ = error;
                ConsistencyInspectionError::DatabaseUnavailable
            })?;
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
        .await;
        match result {
            Ok(Ok(())) => {
                report.complete = true;
                report.consistent = report.checks.iter().all(|check| check.violation_count == 0);
            }
            Ok(Err(())) => report.failure_code = Some("inspection_query_failed"),
            Err(error) => {
                let _ = error;
                report.failure_code = Some("inspection_deadline_exceeded")
            }
        }
        report.completed_at = Utc::now();
        append_report(self, &report).await?;
        Ok(report)
    }
}

async fn inspect_snapshot(
    store: &PostgresStore,
    report: &mut ConsistencyInspectionReport,
) -> Result<(), ()> {
    let mut transaction = store.pool().begin_read_snapshot().await.map_err(|error| {
        let _ = error;
    })?;
    sqlx::query("SET LOCAL statement_timeout = '5s'").execute(&mut *transaction).await.map_err(
        |error| {
            let _ = error;
        },
    )?;
    sqlx::query("SET LOCAL lock_timeout = '1s'").execute(&mut *transaction).await.map_err(
        |error| {
            let _ = error;
        },
    )?;
    let identity = sqlx::query(
        "SELECT transaction_timestamp() AS observed_at, pg_current_snapshot()::text AS snapshot",
    )
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| {
        let _ = error;
    })?;
    report.observed_at = Some(identity.try_get("observed_at").map_err(|error| {
        let _ = error;
    })?);
    report.snapshot = Some(identity.try_get("snapshot").map_err(|error| {
        let _ = error;
    })?);
    for &(invariant, sql) in queries::CHECKS {
        report.interrupted_invariant = Some(invariant);
        // 子查询完整计数，窗口结果至多返回 100 个标识；客户端不载入业务行或正文。
        let rows = sqlx::query(sql).bind(SAMPLE_LIMIT).fetch_all(&mut *transaction).await.map_err(
            |error| {
                let _ = error;
            },
        )?;
        let violation_count = rows
            .first()
            .map(|row| row.try_get::<i64, _>("violation_count"))
            .transpose()
            .map_err(|error| {
                let _ = error;
            })?
            .unwrap_or(0);
        let samples = rows
            .into_iter()
            .map(|row| {
                Ok(ConsistencyViolation {
                    entity_key: row.try_get("entity_key").map_err(|error| {
                        let _ = error;
                    })?,
                    reason: row.try_get("reason").map_err(|error| {
                        let _ = error;
                    })?,
                })
            })
            .collect::<Result<Vec<_>, ()>>()?;
        report.checks.push(ConsistencyCheckReport {
            invariant,
            violation_count,
            samples_truncated: violation_count > SAMPLE_LIMIT,
            samples,
        });
        report.interrupted_invariant = None;
    }
    transaction.commit().await.map_err(|error| {
        let _ = error;
    })
}

async fn append_report(
    store: &PostgresStore,
    report: &ConsistencyInspectionReport,
) -> Result<(), ConsistencyInspectionError> {
    let append = async {
        let mut transaction = store.pool().begin().await.map_err(|error| {
            let _ = error;
        })?;
        sqlx::query("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await
            .map_err(|error| {
                let _ = error;
            })?;
        sqlx::query("SET LOCAL lock_timeout = '1s'").execute(&mut *transaction).await.map_err(
            |error| {
                let _ = error;
            },
        )?;
        let metadata = serde_json::to_value(report).map_err(|error| {
            let _ = error;
        })?;
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
        .await
        .map_err(|error| { let _ = error; })?;
        transaction.commit().await.map_err(|error| {
            let _ = error;
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), append)
        .await
        .map_err(|error| {
            let _ = error;
            ConsistencyInspectionError::ReportPersistence
        })?
        .map_err(|()| ConsistencyInspectionError::ReportPersistence)
}
