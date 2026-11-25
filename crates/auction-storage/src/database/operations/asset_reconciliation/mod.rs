//! 资产对账与偿付能力报告：有界只读快照与只追加报告事实。
//!
//! 报告不授权任何资金或资产动作；Armory 观察与余额读取只来自既有只读端口。未观察到
//! 的装备只表示本轮 Armory 七类 organize 页面没有命中，不等于丢失或可处置。已成功寄出的
//! 装备与已支出（且寄出不晚于余额观察）的货款按持久寄出事实单列为在途：它们已随邮件离开
//! Bot，不是缺失或待付；寄出后履约转人工核对或失败的另列为在途待核对。

use std::time::Duration;

use auction_application::core::ApplicationFuture;
use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::reconciliation::{
    AssetArmoryObservation, AssetReconciliationStore, AssetReportSummary, CustodyInTransitReason,
    CustodyMissingSample, CustodyReconciliationReport, CustodyWorldReport, SolvencyReport,
};
use auction_time::Timestamp;
use serde::Serialize;
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::operations::audit_health::AuditWriteResult as _;

mod consignment;
mod consignment_solvency;
mod queries;
mod solvency;
#[cfg(test)]
mod tests;

/// 只在 `test-db` 下暴露偿付查询原文给基准/EXPLAIN 夹具，保证测量的是同一份 SQL；默认构建与
/// 发布制品不包含该出口。
#[cfg(feature = "test-db")]
pub mod benchmark_sql {
    /// 单个世界的偿付分类查询（`$1` 是 world）。
    pub fn solvency_world_sql() -> &'static str {
        super::queries::SOLVENCY_WORLD
    }
}

/// 每个 world 最多保存的差异样本；总数仍由完整查询计算。
const SAMPLE_LIMIT: i64 = 100;
// 单轮只读快照与追加事务的 tokio 总预算（秒）；单条语句超时由事务内
// `SET LOCAL statement_timeout = '5000ms'` 另行固定，两者不需要相等。
const REPORT_TIMEOUT_SECONDS: u64 = 40;
const APPEND_TIMEOUT_SECONDS: u64 = 10;
/// 报告 world 的固定顺序；与配置校验的递增顺序一致。
const WORLDS: [&str; 2] = ["isekai", "main"];
/// 快照超出 [`REPORT_TIMEOUT_SECONDS`] 时写入报告的稳定失败码。
const DEADLINE_EXCEEDED: &str = "reconciliation_deadline_exceeded";

/// v2 增加在途与在途待核对装备（`in_transit_*`）；v1 报告读取时补 0/空列表。
pub const CUSTODY_REPORT_FORMAT: &str = "auction-custody-reconciliation.v2";
/// v2 增加在途与在途待核对货款与退款（`in_transit_*`），待付不再含它们；v1 报告读取时补 0。
pub const SOLVENCY_REPORT_FORMAT: &str = "auction-solvency-report.v2";
pub const CUSTODY_REPORT_ACTION: &str = "auction.custody.reconcile";
pub const SOLVENCY_REPORT_ACTION: &str = "auction.solvency.report";
/// 每条待发出站邮件的估算邮资。
///
/// 尚未核实 HV 的邮资金额，因此保持 0 并标记 `postage_rate_verified=false`；
/// 报告不得用伪造金额制造偿付缺口或充足结论。
pub const POSTAGE_RATE_CREDITS: i64 = 0;
pub const POSTAGE_RATE_VERIFIED: bool = false;
/// 报告事实的实体类型；与迁移 000095 的部分索引一致。
pub const CUSTODY_REPORT_ENTITY: &str = "custody_report";
pub const SOLVENCY_REPORT_ENTITY: &str = "solvency_report";

impl AssetReconciliationStore for DatabaseStore {
    fn reconcile_custody(
        &self,
        observations: Vec<AssetArmoryObservation>,
    ) -> ApplicationFuture<'_, CustodyReconciliationReport> {
        Box::pin(async move {
            let database_name = current_database(self).await?;
            let mut report = CustodyReconciliationReport {
                format: CUSTODY_REPORT_FORMAT.to_owned(),
                report_id: Uuid::new_v4(),
                database_name,
                completed_at: Timestamp::now(),
                sample_limit_per_world: SAMPLE_LIMIT,
                complete: false,
                consistent: false,
                worlds: Vec::with_capacity(WORLDS.len()),
                failure_code: None,
                asset_actions_authorized: false,
            };
            let result = tokio::time::timeout(
                Duration::from_secs(REPORT_TIMEOUT_SECONDS),
                custody_snapshot(self, &mut report, observations),
            )
            .await
            .unwrap_or(Err(DEADLINE_EXCEEDED));
            match result {
                Ok(()) => {
                    report.complete =
                        report.worlds.iter().all(|world| world.observation == "observed");
                    report.consistent = report.complete
                        && report
                            .worlds
                            .iter()
                            .all(|world| world.missing_count == 0 && world.unexpected_count == 0);
                }
                Err(failure_code) => report.failure_code = Some(failure_code.to_owned()),
            }
            report.completed_at = Timestamp::now();
            append_report(
                self,
                CUSTODY_REPORT_ACTION,
                CUSTODY_REPORT_ENTITY,
                report.report_id,
                if report.consistent { "success" } else { "failure" },
                &report,
            )
            .await?;
            Ok(report)
        })
    }

    fn report_solvency(
        &self,
        balance_evidence_max_age_seconds: u64,
    ) -> ApplicationFuture<'_, SolvencyReport> {
        Box::pin(async move {
            let database_name = current_database(self).await?;
            let mut report = SolvencyReport {
                format: SOLVENCY_REPORT_FORMAT.to_owned(),
                report_id: Uuid::new_v4(),
                database_name,
                completed_at: Timestamp::now(),
                postage_rate_credits: POSTAGE_RATE_CREDITS,
                postage_rate_verified: POSTAGE_RATE_VERIFIED,
                complete: false,
                solvent: false,
                worlds: Vec::with_capacity(WORLDS.len()),
                failure_code: None,
                asset_actions_authorized: false,
            };
            let result = tokio::time::timeout(
                Duration::from_secs(REPORT_TIMEOUT_SECONDS),
                solvency::solvency_snapshot(self, &mut report, balance_evidence_max_age_seconds),
            )
            .await
            .unwrap_or(Err(DEADLINE_EXCEEDED));
            match result {
                Ok(()) => report.complete = true,
                Err(failure_code) => report.failure_code = Some(failure_code.to_owned()),
            }
            report.completed_at = Timestamp::now();
            report.solvent = report.is_solvent();
            append_report(
                self,
                SOLVENCY_REPORT_ACTION,
                SOLVENCY_REPORT_ENTITY,
                report.report_id,
                if report.solvent { "success" } else { "failure" },
                &report,
            )
            .await?;
            Ok(report)
        })
    }

    fn latest_asset_reports(&self) -> ApplicationFuture<'_, AssetReportSummary> {
        Box::pin(async move {
            let rows = sqlx::query(queries::LATEST_REPORTS).fetch_all(self.pool()).await.map_err(
                |error| crate::database::storage_error_from(error, "读取资产报告投影失败"),
            )?;
            let mut summary = AssetReportSummary::default();
            for row in rows {
                let entity_type: String = row.try_get("entity_type").map_err(projection_failed)?;
                let metadata: Value = row.try_get("metadata").map_err(projection_failed)?;
                match entity_type.as_str() {
                    CUSTODY_REPORT_ENTITY => {
                        summary.custody =
                            Some(serde_json::from_value(metadata).map_err(report_undecodable)?);
                    }
                    SOLVENCY_REPORT_ENTITY => {
                        summary.solvency =
                            Some(serde_json::from_value(metadata).map_err(report_undecodable)?);
                    }
                    _ => return Err(report_projection_error()),
                }
            }
            Ok(summary)
        })
    }
}

fn report_projection_error() -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed("资产报告投影无法解析".to_owned()))
}

fn projection_failed(error: sqlx::Error) -> ApplicationError {
    crate::database::storage_error_from(error, "资产报告投影无法解析")
}

/// serde 错误可能回显报告内容，只归类不记录。
fn report_undecodable(_: serde_json::Error) -> ApplicationError {
    report_projection_error()
}

/// 查询或列解码失败：SQLSTATE 等安全上下文进日志，报告只保留稳定失败码。
fn query_failed(error: sqlx::Error) -> &'static str {
    failure_code(&error, "reconciliation_query_failed")
}

fn snapshot_unavailable(error: sqlx::Error) -> &'static str {
    failure_code(&error, "reconciliation_snapshot_unavailable")
}

fn failure_code(error: &sqlx::Error, code: &'static str) -> &'static str {
    let context = crate::database::sql_error_context(error, "资产对账只读快照失败");
    tracing::warn!(kind = code, phase = "asset_reconciliation", context, "资产对账未完成");
    code
}

async fn current_database(store: &DatabaseStore) -> Result<String, ApplicationError> {
    sqlx::query_scalar("SELECT current_database()")
        .fetch_one(store.pool())
        .await
        .map_err(|error| crate::database::storage_error_from(error, "资产对账数据库连接不可用"))
}

async fn open_snapshot(
    store: &DatabaseStore,
) -> Result<crate::database::pool::TrackedTransaction<'static, Postgres>, &'static str> {
    let mut transaction = store.pool().begin_read_snapshot().await.map_err(snapshot_unavailable)?;
    sqlx::query("SET LOCAL statement_timeout = '5000ms'")
        .execute(&mut *transaction)
        .await
        .map_err(snapshot_unavailable)?;
    sqlx::query("SET LOCAL lock_timeout = '1s'")
        .execute(&mut *transaction)
        .await
        .map_err(snapshot_unavailable)?;
    Ok(transaction)
}

async fn custody_snapshot(
    store: &DatabaseStore,
    report: &mut CustodyReconciliationReport,
    observations: Vec<AssetArmoryObservation>,
) -> Result<(), &'static str> {
    let mut transaction = open_snapshot(store).await?;
    for world in WORLDS {
        let observation =
            observations.iter().find(|observation| observation.world.as_str() == world).cloned();
        let mut world_report = CustodyWorldReport {
            world: world.to_owned(),
            observation: "unavailable".to_owned(),
            expected_count: 0,
            observed_count: 0,
            missing_count: 0,
            in_transit_count: 0,
            in_transit_reasons: Vec::new(),
            in_transit_review_count: 0,
            in_transit_review_reasons: Vec::new(),
            stock_count: 0,
            personal_count: 0,
            unexpected_count: 0,
            missing_samples: Vec::new(),
            unexpected_samples: Vec::new(),
            samples_truncated: false,
            failure_code: None,
        };
        let Some(observation) = observation.filter(AssetArmoryObservation::validate) else {
            world_report.failure_code = Some("inventory_observation_unavailable".to_owned());
            report.worlds.push(world_report);
            continue;
        };
        if !observation.complete {
            world_report.observation = observation.status().to_owned();
            world_report.failure_code = observation
                .failure_code
                .map(ToOwned::to_owned)
                .or_else(|| Some("inventory_observation_unavailable".to_owned()));
            report.worlds.push(world_report);
            continue;
        }
        let ids = &observation.equipment_ids;
        let counts = sqlx::query(queries::CUSTODY_COUNTS)
            .bind(world)
            .bind(ids)
            .bind(SAMPLE_LIMIT)
            .fetch_one(&mut *transaction)
            .await
            .map_err(query_failed)?;
        world_report.observation = "observed".to_owned();
        world_report.expected_count =
            counts.try_get::<i64, _>("expected_count").map_err(query_failed)?;
        world_report.observed_count =
            counts.try_get::<i64, _>("observed_count").map_err(query_failed)?;
        world_report.missing_count =
            counts.try_get::<i64, _>("missing_count").map_err(query_failed)?;
        world_report.stock_count = counts.try_get::<i64, _>("stock_count").map_err(query_failed)?;
        world_report.personal_count =
            counts.try_get::<i64, _>("personal_count").map_err(query_failed)?;
        world_report.unexpected_count =
            counts.try_get::<i64, _>("unexpected_count").map_err(query_failed)?;
        let samples = sqlx::query(queries::CUSTODY_SAMPLES)
            .bind(world)
            .bind(ids)
            .bind(SAMPLE_LIMIT)
            .fetch_all(&mut *transaction)
            .await
            .map_err(query_failed)?;
        // `unexpected_samples` 是 v1 报告的保留字段，保持为空。
        for row in samples {
            world_report.missing_samples.push(CustodyMissingSample {
                external_equipment_id: row
                    .try_get("external_equipment_id")
                    .map_err(query_failed)?,
                reason: row.try_get("reason").map_err(query_failed)?,
            });
        }
        world_report.samples_truncated = world_report.missing_count
            > i64::try_from(world_report.missing_samples.len()).unwrap_or(0)
            || world_report.unexpected_count
                > i64::try_from(world_report.unexpected_samples.len()).unwrap_or(0);
        let in_transit = sqlx::query(queries::CUSTODY_IN_TRANSIT)
            .bind(world)
            .bind(ids)
            .fetch_all(&mut *transaction)
            .await
            .map_err(query_failed)?;
        for row in in_transit {
            let review: bool = row.try_get("review").map_err(query_failed)?;
            let count: i64 = row.try_get("item_count").map_err(query_failed)?;
            let (total, reasons) = if review {
                (
                    &mut world_report.in_transit_review_count,
                    &mut world_report.in_transit_review_reasons,
                )
            } else {
                (&mut world_report.in_transit_count, &mut world_report.in_transit_reasons)
            };
            *total = total.checked_add(count).ok_or("reconciliation_query_failed")?;
            reasons.push(CustodyInTransitReason {
                reason: row.try_get("reason").map_err(query_failed)?,
                count,
            });
        }
        report.worlds.push(world_report);
    }
    transaction.commit().await.map_err(snapshot_unavailable)
}

async fn append_report<T: Serialize>(
    store: &DatabaseStore,
    action: &'static str,
    entity_type: &'static str,
    report_id: Uuid,
    result: &'static str,
    report: &T,
) -> Result<(), ApplicationError> {
    let metadata = serde_json::to_value(report).map_err(report_undecodable)?;
    let append = async {
        let mut transaction = store.pool().begin().await?;
        sqlx::query("SET LOCAL statement_timeout = '5000ms'").execute(&mut *transaction).await?;
        sqlx::query("SET LOCAL lock_timeout = '1s'").execute(&mut *transaction).await?;
        sqlx::query(
            "INSERT INTO public.audit_records
                 (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
             VALUES ($1, 'worker', 'auction-reconciliation', $2, $3, $4, $5, 'worker', $6)",
        )
        .bind(Uuid::new_v4())
        .bind(action)
        .bind(entity_type)
        .bind(report_id.to_string())
        .bind(result)
        .bind(metadata)
        .execute(&mut *transaction)
        .await
        .record_audit_failure()?;
        transaction.commit().await
    };
    tokio::time::timeout(Duration::from_secs(APPEND_TIMEOUT_SECONDS), append)
        .await
        .map_err(append_timed_out)?
        .map_err(|error| crate::database::storage_error_from(error, "资产报告追加保存失败"))
}

fn append_timed_out(_: tokio::time::error::Elapsed) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed("资产报告追加保存超时".to_owned()))
}
