//! Admin dashboard operations alert section。

use auction_application::OperationsAlertHealth;
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::json;

use super::super::super::timestamp;

pub(crate) fn map_alerting(health: OperationsAlertHealth) -> AdminDashboardSectionResponse {
    let status = if !health.enabled {
        DashboardStatus::Unknown
    } else if health.failed_batch_count > 0
        || health.manual_reconcile_count > 0
        || health.claimed_batch_count > 0
        || health.due_signal_count > 0
    {
        DashboardStatus::Degraded
    } else {
        DashboardStatus::Ok
    };
    let error_code = if health.manual_reconcile_count > 0 {
        Some("manual_reconcile".to_owned())
    } else if health.failed_batch_count > 0 {
        Some("delivery_failed".to_owned())
    } else if !health.last_error_code.is_empty() {
        Some(health.last_error_code.clone())
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: status.as_str().to_owned(),
        observed_at: health.last_observed_at.map(timestamp),
        error_code,
        data: Some(json!({
            "enabled": health.enabled,
            "active_signal_count": health.active_signal_count,
            "due_signal_count": health.due_signal_count,
            "claimed_batch_count": health.claimed_batch_count,
            "succeeded_batch_count": health.succeeded_batch_count,
            "failed_batch_count": health.failed_batch_count,
            "manual_reconcile_count": health.manual_reconcile_count,
            "last_observed_at": health.last_observed_at.map(timestamp),
            "last_success_at": health.last_success_at.map(timestamp),
            "last_failure_at": health.last_failure_at.map(timestamp),
            "last_error_code": health.last_error_code,
        })),
    }
}
