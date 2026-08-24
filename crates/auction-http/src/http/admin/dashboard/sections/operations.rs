//! Admin dashboard 履约、投递和 recovery 水位 section。

use auction_application::{
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot,
    OperationsAlertRecoverySnapshot,
};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::json;

use super::super::super::timestamp;

pub(crate) fn map_fulfillment(
    snapshot: OperationsAlertFulfillmentSnapshot,
) -> AdminDashboardSectionResponse {
    let error_code = if snapshot.manual_reconcile > 0 {
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
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": timestamp(chrono::Utc::now()),
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
    let error_code = if snapshot.manual_reconcile > 0 {
        Some("manual_reconcile")
    } else if snapshot.failed > 0 {
        Some("delivery_failed")
    } else if snapshot.pending > 0 {
        Some("delivery_backlog")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": timestamp(chrono::Utc::now()),
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
    let error_code = if snapshot.manual_reconcile_count > 0 {
        Some("manual_reconcile")
    } else if snapshot.failed_action_count > 0 {
        Some("recovery_action_failed")
    } else if snapshot.payment_overdue_count > 0 {
        Some("payment_overdue")
    } else if snapshot.relist_pending_count > 0 {
        Some("relist_pending")
    } else if snapshot
        .worlds
        .iter()
        .any(|world| world.balance_status != "ok" || world.evidence_status != "ok")
    {
        Some("recovery_source_degraded")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": timestamp(chrono::Utc::now()),
            "worlds": snapshot.worlds.into_iter().map(|world| json!({
                "world": world.world,
                "balance_credits": world.balance_credits,
                "balance_status": world.balance_status,
                "evidence_status": world.evidence_status,
                "evidence_age_seconds": world.evidence_age_seconds,
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
