//! W11.1–W11.3 资产对账与偿付能力报告：有界只读快照与只追加报告事实。
//!
//! 报告不授权任何资金或资产动作；Armory 观察与余额读取只来自既有只读端口。未观察到
//! 的装备只表示本轮 Armory 七类 organize 页面没有命中，不等于丢失或可处置。

use std::time::Duration;

use auction_application::auction::ApplicationFuture;
use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::reconciliation::{
    AssetArmoryObservation, AssetReconciliationStore, AssetReportSummary, CustodyMissingSample,
    CustodyReconciliationReport, CustodyWorldReport, SolvencyReport, SolvencyWorldReport,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::{DatabaseStore, PostgresStore};

mod queries;
#[cfg(test)]
mod tests;

/// 每个 world 最多保存的差异样本；总数仍由完整查询计算。
const SAMPLE_LIMIT: i64 = 100;
// 每个只读快照与追加事务的语句超时；值必须与下方 SET LOCAL 字面量一致。
const REPORT_TIMEOUT_SECONDS: u64 = 40;
const APPEND_TIMEOUT_SECONDS: u64 = 10;
/// 报告 world 的固定顺序；与配置校验的递增顺序一致。
const WORLDS: [&str; 2] = ["isekai", "main"];

pub const CUSTODY_REPORT_FORMAT: &str = "auction-custody-reconciliation.v1";
pub const SOLVENCY_REPORT_FORMAT: &str = "auction-solvency-report.v1";
pub const CUSTODY_REPORT_ACTION: &str = "auction.custody.reconcile";
pub const SOLVENCY_REPORT_ACTION: &str = "auction.solvency.report";
/// 每条待发出站邮件的估算邮资。
///
/// W0.12 尚未核实 HV 的邮资金额，因此保持 0 并标记 `postage_rate_verified=false`；
/// 报告不得用伪造金额制造偿付缺口或充足结论。
pub const POSTAGE_RATE_CREDITS: i64 = 0;
pub const POSTAGE_RATE_VERIFIED: bool = false;
/// 报告事实的实体类型；与迁移 000095 的部分索引一致。
pub const CUSTODY_REPORT_ENTITY: &str = "custody_report";
pub const SOLVENCY_REPORT_ENTITY: &str = "solvency_report";

impl AssetReconciliationStore for PostgresStore {
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
                completed_at: Utc::now(),
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
            .await;
            match result {
                Ok(Ok(())) => {
                    report.complete =
                        report.worlds.iter().all(|world| world.observation == "observed");
                    report.consistent = report.complete
                        && report
                            .worlds
                            .iter()
                            .all(|world| world.missing_count == 0 && world.unexpected_count == 0);
                }
                Ok(Err(failure_code)) => report.failure_code = Some(failure_code.to_owned()),
                Err(error) => {
                    let _ = error;
                    report.failure_code = Some("reconciliation_deadline_exceeded".to_owned());
                }
            }
            report.completed_at = Utc::now();
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

    fn report_solvency(&self) -> ApplicationFuture<'_, SolvencyReport> {
        Box::pin(async move {
            let database_name = current_database(self).await?;
            let mut report = SolvencyReport {
                format: SOLVENCY_REPORT_FORMAT.to_owned(),
                report_id: Uuid::new_v4(),
                database_name,
                completed_at: Utc::now(),
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
                solvency_snapshot(self, &mut report),
            )
            .await;
            match result {
                Ok(Ok(())) => report.complete = true,
                Ok(Err(failure_code)) => report.failure_code = Some(failure_code.to_owned()),
                Err(error) => {
                    let _ = error;
                    report.failure_code = Some("reconciliation_deadline_exceeded".to_owned());
                }
            }
            report.completed_at = Utc::now();
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
                |error| {
                    let _ = error;
                    ApplicationError::Repository(RepositoryError::Failed(
                        "读取资产报告投影失败".to_owned(),
                    ))
                },
            )?;
            let mut summary = AssetReportSummary::default();
            for row in rows {
                let entity_type: String = row.try_get("entity_type").map_err(|error| {
                    let _ = error;
                    report_projection_error()
                })?;
                let metadata: Value = row.try_get("metadata").map_err(|error| {
                    let _ = error;
                    report_projection_error()
                })?;
                match entity_type.as_str() {
                    CUSTODY_REPORT_ENTITY => {
                        summary.custody =
                            Some(serde_json::from_value(metadata).map_err(|error| {
                                let _ = error;
                                report_projection_error()
                            })?);
                    }
                    SOLVENCY_REPORT_ENTITY => {
                        summary.solvency =
                            Some(serde_json::from_value(metadata).map_err(|error| {
                                let _ = error;
                                report_projection_error()
                            })?);
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

async fn current_database(store: &PostgresStore) -> Result<String, ApplicationError> {
    sqlx::query_scalar("SELECT current_database()").fetch_one(store.pool()).await.map_err(|error| {
        let _ = error;
        ApplicationError::Repository(RepositoryError::Failed("资产对账数据库连接不可用".to_owned()))
    })
}

async fn open_snapshot(
    store: &PostgresStore,
) -> Result<crate::database::pool::TrackedTransaction<'static, Postgres>, &'static str> {
    let mut transaction = store.pool().begin_read_snapshot().await.map_err(|error| {
        let _ = error;
        "reconciliation_snapshot_unavailable"
    })?;
    sqlx::query("SET LOCAL statement_timeout = '5000ms'")
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            let _ = error;
            "reconciliation_snapshot_unavailable"
        })?;
    sqlx::query("SET LOCAL lock_timeout = '1s'").execute(&mut *transaction).await.map_err(
        |error| {
            let _ = error;
            "reconciliation_snapshot_unavailable"
        },
    )?;
    Ok(transaction)
}

async fn custody_snapshot(
    store: &PostgresStore,
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
            .map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        world_report.observation = "observed".to_owned();
        world_report.expected_count =
            counts.try_get::<i64, _>("expected_count").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        world_report.observed_count =
            counts.try_get::<i64, _>("observed_count").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        world_report.missing_count =
            counts.try_get::<i64, _>("missing_count").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        world_report.stock_count = counts.try_get::<i64, _>("stock_count").map_err(|error| {
            let _ = error;
            "reconciliation_query_failed"
        })?;
        world_report.personal_count =
            counts.try_get::<i64, _>("personal_count").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        world_report.unexpected_count =
            counts.try_get::<i64, _>("unexpected_count").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        let samples = sqlx::query(queries::CUSTODY_SAMPLES)
            .bind(world)
            .bind(ids)
            .bind(SAMPLE_LIMIT)
            .fetch_all(&mut *transaction)
            .await
            .map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        for row in samples {
            let kind: String = row.try_get("kind").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
            let external_equipment_id: String =
                row.try_get("external_equipment_id").map_err(|error| {
                    let _ = error;
                    "reconciliation_query_failed"
                })?;
            match kind.as_str() {
                "missing" => world_report.missing_samples.push(CustodyMissingSample {
                    external_equipment_id,
                    reason: row.try_get("reason").map_err(|error| {
                        let _ = error;
                        "reconciliation_query_failed"
                    })?,
                }),
                "unexpected" => world_report.unexpected_samples.push(external_equipment_id),
                _ => return Err("reconciliation_query_failed"),
            }
        }
        world_report.samples_truncated = world_report.missing_count
            > i64::try_from(world_report.missing_samples.len()).unwrap_or(0)
            || world_report.unexpected_count
                > i64::try_from(world_report.unexpected_samples.len()).unwrap_or(0);
        report.worlds.push(world_report);
    }
    transaction.commit().await.map_err(|error| {
        let _ = error;
        "reconciliation_snapshot_unavailable"
    })
}

async fn solvency_snapshot(
    store: &PostgresStore,
    report: &mut SolvencyReport,
) -> Result<(), &'static str> {
    let mut transaction = open_snapshot(store).await?;
    for world in WORLDS {
        let row = sqlx::query(queries::SOLVENCY_WORLD)
            .bind(world)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        let pending_seller_payout_count: i64 =
            row.try_get("pending_seller_payout_count").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        let pending_seller_payout_credits: i64 =
            row.try_get("pending_seller_payout_credits").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        let pending_refund_count: i64 = row.try_get("pending_refund_count").map_err(|error| {
            let _ = error;
            "reconciliation_query_failed"
        })?;
        let pending_refund_credits: i64 =
            row.try_get("pending_refund_credits").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        let pending_outbound_mail_count: i64 =
            row.try_get("pending_outbound_mail_count").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        let postage_estimate_credits = pending_outbound_mail_count
            .checked_mul(report.postage_rate_credits)
            .ok_or("reconciliation_query_failed")?;
        let required_credits = pending_seller_payout_credits
            .checked_add(pending_refund_credits)
            .and_then(|total| total.checked_add(postage_estimate_credits))
            .ok_or("reconciliation_query_failed")?;
        let balance_credits: Option<i64> = row.try_get("balance_credits").map_err(|error| {
            let _ = error;
            "reconciliation_query_failed"
        })?;
        let balance_observed_at: Option<DateTime<Utc>> =
            row.try_get("balance_observed_at").map_err(|error| {
                let _ = error;
                "reconciliation_query_failed"
            })?;
        let balance_status: Option<String> = row.try_get("balance_status").map_err(|error| {
            let _ = error;
            "reconciliation_query_failed"
        })?;
        let evidence_status: Option<String> = row.try_get("evidence_status").map_err(|error| {
            let _ = error;
            "reconciliation_query_failed"
        })?;
        let balance_status = balance_status.unwrap_or_else(|| "unknown".to_owned());
        let evidence_status = evidence_status.unwrap_or_else(|| "unknown".to_owned());
        let (verdict, shortfall_credits) =
            solvency_verdict(required_credits, balance_credits, &balance_status, &evidence_status);
        report.worlds.push(SolvencyWorldReport {
            world: world.to_owned(),
            pending_seller_payout_count,
            pending_seller_payout_credits,
            pending_refund_count,
            pending_refund_credits,
            pending_outbound_mail_count,
            postage_estimate_credits,
            required_credits,
            balance_credits,
            balance_observed_at,
            balance_status,
            evidence_status,
            verdict: verdict.to_owned(),
            shortfall_credits,
        });
    }
    transaction.commit().await.map_err(|error| {
        let _ = error;
        "reconciliation_snapshot_unavailable"
    })
}

/// 余额已被成功观察且未过期时才允许判定；其余情况一律 `unknown`，不以缺失值当作足够。
pub(crate) fn solvency_verdict(
    required_credits: i64,
    balance_credits: Option<i64>,
    balance_status: &str,
    evidence_status: &str,
) -> (&'static str, i64) {
    let fresh = matches!(balance_status, "ok" | "low") && evidence_status == "fresh";
    match balance_credits.filter(|credits| *credits >= 0).filter(|_| fresh) {
        Some(balance) => {
            let shortfall = required_credits.saturating_sub(balance);
            if shortfall > 0 { ("shortfall", shortfall) } else { ("sufficient", 0) }
        }
        None => ("unknown", 0),
    }
}

async fn append_report<T: Serialize>(
    store: &PostgresStore,
    action: &'static str,
    entity_type: &'static str,
    report_id: Uuid,
    result: &'static str,
    report: &T,
) -> Result<(), ApplicationError> {
    let metadata = serde_json::to_value(report).map_err(|error| {
        let _ = error;
        report_projection_error()
    })?;
    let append = async {
        let mut transaction = store.pool().begin().await.map_err(|error| {
            let _ = error;
        })?;
        sqlx::query("SET LOCAL statement_timeout = '5000ms'")
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
        .map_err(|error| {
            let _ = error;
        })?;
        transaction.commit().await.map_err(|error| {
            let _ = error;
        })
    };
    tokio::time::timeout(Duration::from_secs(APPEND_TIMEOUT_SECONDS), append)
        .await
        .map_err(|error| {
            let _ = error;
            ApplicationError::Repository(RepositoryError::Failed("资产报告追加保存超时".to_owned()))
        })?
        .map_err(|()| {
            ApplicationError::Repository(RepositoryError::Failed("资产报告追加保存失败".to_owned()))
        })
}

/// 数据库枚举包装的委托：生产装配以 `Arc<DatabaseStore>` 注入，唯一后端为 PostgreSQL。
impl AssetReconciliationStore for DatabaseStore {
    fn reconcile_custody(
        &self,
        observations: Vec<AssetArmoryObservation>,
    ) -> ApplicationFuture<'_, CustodyReconciliationReport> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.reconcile_custody(observations),
        }
    }

    fn report_solvency(&self) -> ApplicationFuture<'_, SolvencyReport> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.report_solvency(),
        }
    }

    fn latest_asset_reports(&self) -> ApplicationFuture<'_, AssetReportSummary> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(store) => store.latest_asset_reports(),
        }
    }
}
