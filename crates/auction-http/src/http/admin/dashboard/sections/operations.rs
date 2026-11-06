//! Admin dashboard 履约、投递和 recovery 水位 section。

use auction_application::{
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot,
    OperationsAlertReconciliationSnapshot, OperationsAlertRecoverySnapshot,
};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::json;

use super::super::super::timestamp;

pub(crate) fn map_fulfillment(
    snapshot: OperationsAlertFulfillmentSnapshot,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(chrono::Utc::now());
    let degraded_reason = if snapshot.manual_reconcile > 0 {
        Some("manual_reconcile")
    } else if snapshot.failed > 0 {
        Some("fulfillment_failed")
    } else if snapshot.blocked > 0 {
        Some("fulfillment_blocked")
    } else if snapshot.pending > 0 {
        Some("fulfillment_backlog")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if degraded_reason.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "pending": snapshot.pending,
            "blocked": snapshot.blocked,
            "failed": snapshot.failed,
            "manual_reconcile": snapshot.manual_reconcile,
            "oldest_backlog_age_seconds": snapshot.oldest_backlog_age_seconds,
        })),
    }
}

pub(crate) fn map_delivery(
    snapshot: OperationsAlertDeliverySnapshot,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(chrono::Utc::now());
    let degraded_reason = if snapshot.manual_reconcile > 0 {
        Some("manual_reconcile")
    } else if snapshot.failed > 0 {
        Some("delivery_failed")
    } else if snapshot.pending > 0 {
        Some("delivery_backlog")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if degraded_reason.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "water": {
                "pending": snapshot.pending,
                "failed": snapshot.failed,
                "manual_reconcile": snapshot.manual_reconcile,
                "oldest_pending_age_seconds": snapshot.oldest_pending_age_seconds,
                "oldest_manual_reconcile_age_seconds": snapshot.oldest_manual_reconcile_age_seconds,
                "last_update_at": snapshot.last_update_at.map(timestamp),
            },
        })),
    }
}

pub(crate) fn map_recovery(
    snapshot: OperationsAlertRecoverySnapshot,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(chrono::Utc::now());
    let degraded_reason = if snapshot.manual_reconcile_count > 0 {
        Some("manual_reconcile")
    } else if snapshot.failed_action_count > 0 {
        Some("recovery_action_failed")
    } else if snapshot.payment_overdue_count > 0 {
        Some("payment_overdue")
    } else if snapshot.relist_pending_count > 0 {
        Some("relist_pending")
    } else if snapshot.worlds.iter().any(|world| {
        // 只记账策略将两项现金来源同时标为不适用；混合或未知状态仍需告警。
        !matches!(
            (world.balance_status.as_str(), world.evidence_status.as_str()),
            ("ok", "fresh") | ("not_applicable", "not_applicable")
        )
    }) {
        Some("recovery_source_degraded")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if degraded_reason.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "worlds": snapshot.worlds.into_iter().map(|world| json!({
                "world": world.world,
                "balance_credits": world.balance_credits,
                "balance_status": world.balance_status,
                "evidence_status": world.evidence_status,
                "balance_observed_at": world.balance_observed_at.map(timestamp),
                "evidence_last_success_at": world.evidence_last_success_at.map(timestamp),
                "updated_at": timestamp(world.updated_at),
            })).collect::<Vec<_>>(),
            "payment_overdue_count": snapshot.payment_overdue_count,
            "relist_pending_count": snapshot.relist_pending_count,
            "failed_action_count": snapshot.failed_action_count,
            "manual_reconcile_count": snapshot.manual_reconcile_count,
            "oldest_overdue_age_seconds": snapshot.oldest_overdue_age_seconds,
        })),
    }
}

/// W11.1–W11.3 资产对账与偿付能力 section；报告缺失时明确 unknown，不用空数据冒充无差异。
pub(crate) fn map_reconciliation(
    snapshot: OperationsAlertReconciliationSnapshot,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(chrono::Utc::now());
    let (Some(custody_report_age_seconds), Some(solvency_report_age_seconds)) =
        (snapshot.custody_report_age_seconds, snapshot.solvency_report_age_seconds)
    else {
        return AdminDashboardSectionResponse {
            status: DashboardStatus::Unknown.as_str().to_owned(),
            observed_at: None,
            error_code: Some("reconciliation_report_missing".to_owned()),
            data: None,
        };
    };
    let drifted = snapshot
        .worlds
        .iter()
        .any(|world| world.custody_missing_count > 0 || world.solvency_shortfall_credits > 0);
    AdminDashboardSectionResponse {
        status: if drifted {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "custody_report_age_seconds": custody_report_age_seconds,
            "solvency_report_age_seconds": solvency_report_age_seconds,
            "worlds": snapshot.worlds.iter().map(|world| json!({
                "world": world.world,
                "custody_missing_count": world.custody_missing_count,
                "solvency_shortfall_credits": world.solvency_shortfall_credits,
            })).collect::<Vec<_>>(),
        })),
    }
}
