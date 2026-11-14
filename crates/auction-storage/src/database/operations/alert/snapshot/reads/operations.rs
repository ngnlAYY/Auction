//! 进程、生命周期与资产对账面的水位快照。

use super::age_seconds;
use crate::database::DatabaseStore;
use crate::database::operations::alert::storage_error;
use crate::database::storage_error_from;
use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertLifecycleSnapshot, OperationsAlertProcessSnapshot, OperationsAlertProcessState,
};
use auction_application::reconciliation::AssetReconciliationStore;
use auction_observability::{HeartbeatStatus, HeartbeatStore, ServiceName, TracingStatus};
use chrono::{DateTime, Utc};
use sqlx::Row;

pub(super) async fn process_snapshot(
    heartbeat_store: &DatabaseStore,
) -> Result<OperationsAlertProcessSnapshot, ApplicationError> {
    // 复用 health owner 的安全心跳读取；这里不读取 heartbeat details。
    let heartbeats = HeartbeatStore::list_live(heartbeat_store)
        .await
        .map_err(|_| storage_error("读取 operations alert 进程健康快照失败"))?;
    let mut process_states: Vec<Option<OperationsAlertProcessState>> = vec![None, None, None];
    let mut audit_failure_count = 0_i64;
    for heartbeat in heartbeats {
        if !heartbeat.validate() {
            return Err(storage_error("operations alert 进程心跳安全投影无效"));
        }
        audit_failure_count = audit_failure_count
            .checked_add(heartbeat.audit_failure_count)
            .ok_or_else(|| storage_error("operations alert 审计计数溢出"))?;
        let index = match heartbeat.service_name {
            ServiceName::Public => 0,
            ServiceName::Admin => 1,
            ServiceName::Worker => 2,
        };
        let role = heartbeat.process_role.as_str().to_owned();
        let status = if heartbeat.status == HeartbeatStatus::Ready
            && heartbeat.readiness
            && heartbeat.tracing.status != TracingStatus::Degraded
        {
            "ready"
        } else {
            "degraded"
        };
        let replace = process_states[index]
            .as_ref()
            .is_none_or(|current| current.status == "ready" && status == "degraded");
        if replace {
            process_states[index] =
                Some(OperationsAlertProcessState { role, status: status.to_owned() });
        }
    }
    let roles = ["public", "admin", "worker"];
    let processes = process_states
        .into_iter()
        .enumerate()
        .map(|(index, state)| {
            state.unwrap_or_else(|| OperationsAlertProcessState {
                role: roles[index].to_owned(),
                status: "missing".to_owned(),
            })
        })
        .collect();
    Ok(OperationsAlertProcessSnapshot { processes, audit_failure_count })
}

pub(super) async fn lifecycle_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertLifecycleSnapshot, ApplicationError> {
    // 两个 owner 事实：结束时间已过的 live 场次（应进入 Final），以及到期未执行的慢速代理。
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM public.auctions
                WHERE status = 'live' AND effective_end_at IS NOT NULL AND effective_end_at < $1)
                ::bigint AS phase_overdue,
            COALESCE((SELECT EXTRACT(EPOCH FROM ($1::timestamptz - MIN(effective_end_at)))::bigint
                FROM public.auctions
                WHERE status = 'live' AND effective_end_at IS NOT NULL
                  AND effective_end_at < $1), 0) AS oldest_phase_age,
            (SELECT COUNT(*) FROM public.lots
                WHERE status = 'live' AND slow_due_at IS NOT NULL AND slow_due_at < $1)
                ::bigint AS proxy_overdue,
            COALESCE((SELECT EXTRACT(EPOCH FROM ($1::timestamptz - MIN(slow_due_at)))::bigint
                FROM public.lots
                WHERE status = 'live' AND slow_due_at IS NOT NULL AND slow_due_at < $1), 0)
                AS oldest_proxy_age,
            (SELECT COUNT(DISTINCT obligation.id)
             FROM (
                 SELECT MIN(auction.intake_opens_at) AS at
                 FROM public.auctions AS auction
                 WHERE auction.world_id = 'isekai'
                   AND auction.status <> 'cancelled'
                   AND auction.intake_opens_at > $1
                   AND auction.intake_opens_at <= $1 + interval '7 days'
             ) AS boundary
             JOIN public.settlement_obligations AS obligation
               ON obligation.world_id = 'isekai'
             LEFT JOIN public.settlement_obligation_fulfillments AS fulfillment
               ON fulfillment.obligation_id = obligation.id
             WHERE boundary.at IS NOT NULL
               -- 未触发的 contingent 退款不是当前欠付；failed/manual_reconcile 仍需人工收口。
               -- 装备损失线下补偿已由不可变处置事实收口，不计入换季欠付。
               AND obligation.business_state <> 'contingent'
               AND COALESCE(fulfillment.execution_state, 'pending')
                   IN ('pending', 'prepared', 'enqueued', 'failed', 'manual_reconcile')
               AND NOT (
                   COALESCE(fulfillment.execution_state, 'pending') = 'failed'
                   AND fulfillment.last_error_code = 'operator_external_buyer_compensation'
                   AND EXISTS (
                       SELECT 1
                       FROM public.settlement_equipment_losses AS loss
                       WHERE loss.refund_obligation_id = obligation.id
                   )
               )) AS season_obligations_pending",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 生命周期水位失败"))?;
    Ok(OperationsAlertLifecycleSnapshot {
        phase_overdue_count: row
            .try_get("phase_overdue")
            .map_err(|error| storage_error_from(error, "读取场次逾期计数失败"))?,
        oldest_phase_overdue_age_seconds: row
            .try_get("oldest_phase_age")
            .map_err(|error| storage_error_from(error, "读取场次逾期时间失败"))?,
        proxy_expiry_overdue_count: row
            .try_get("proxy_overdue")
            .map_err(|error| storage_error_from(error, "读取慢速代理逾期计数失败"))?,
        oldest_proxy_overdue_age_seconds: row
            .try_get("oldest_proxy_age")
            .map_err(|error| storage_error_from(error, "读取慢速代理逾期时间失败"))?,
        season_obligations_pending: row
            .try_get("season_obligations_pending")
            .map_err(|error| storage_error_from(error, "读取换季未完成义务计数失败"))?,
    })
}

/// 只读取最近追加的两类报告；报告缺失时保持 `None`，evaluator 明确降级。
pub(super) async fn reconciliation_snapshot(
    store: &crate::database::DatabaseStore,
    now: DateTime<Utc>,
) -> Result<auction_application::operations::OperationsAlertReconciliationSnapshot, ApplicationError>
{
    let reports = store.latest_asset_reports().await?;
    let custody_report_age_seconds =
        reports.custody.as_ref().map(|report| age_seconds(now, Some(report.completed_at)));
    let solvency_report_age_seconds =
        reports.solvency.as_ref().map(|report| age_seconds(now, Some(report.completed_at)));
    let mut worlds = Vec::with_capacity(2);
    for world in ["isekai", "main"] {
        let custody_missing_count = reports
            .custody
            .as_ref()
            .and_then(|report| report.worlds.iter().find(|entry| entry.world == world))
            .map_or(0, |entry| entry.missing_count);
        let solvency_shortfall_credits = reports
            .solvency
            .as_ref()
            .and_then(|report| report.worlds.iter().find(|entry| entry.world == world))
            .map_or(0, |entry| entry.shortfall_credits);
        worlds.push(auction_application::operations::OperationsAlertReconciliationWorld {
            world,
            custody_missing_count,
            solvency_shortfall_credits,
        });
    }
    Ok(auction_application::operations::OperationsAlertReconciliationSnapshot {
        custody_report_age_seconds,
        solvency_report_age_seconds,
        worlds,
    })
}
