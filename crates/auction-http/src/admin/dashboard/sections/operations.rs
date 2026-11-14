//! Admin dashboard 履约、投递和 recovery 水位 section。

use auction_application::operations::{
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
    OperationsAlertReconciliationSnapshot, OperationsAlertRecoverySnapshot, assess_delivery,
    assess_fulfillment, assess_recovery,
};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::json;

use super::super::super::timestamp;

pub(crate) fn map_fulfillment(
    snapshot: OperationsAlertFulfillmentSnapshot,
    policy: Option<&OperationsAlertPolicy>,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(chrono::Utc::now());
    let assessment = policy.map(|policy| assess_fulfillment(snapshot, policy));
    let degraded_reason = if assessment.is_some_and(|assessment| assessment.manual_reconcile) {
        Some("manual_reconcile")
    } else if assessment.is_some_and(|assessment| assessment.failed) {
        Some("fulfillment_failed")
    } else if assessment.is_some_and(|assessment| assessment.backlog) {
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
            "waiting_payment": snapshot.waiting_payment,
            "waiting_prerequisite": snapshot.waiting_prerequisite,
            "oldest_backlog_age_seconds": snapshot.oldest_backlog_age_seconds,
            "oldest_prerequisite_wait_age_seconds": snapshot.oldest_prerequisite_wait_age_seconds,
        })),
    }
}

pub(crate) fn map_delivery(
    snapshot: OperationsAlertDeliverySnapshot,
    policy: Option<&OperationsAlertPolicy>,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(chrono::Utc::now());
    let assessment = policy.map(|policy| assess_delivery(snapshot, policy));
    let degraded_reason = if assessment.is_some_and(|assessment| assessment.manual_reconcile) {
        Some("manual_reconcile")
    } else if assessment.is_some_and(|assessment| assessment.returned_mail) {
        Some("delivery_returned_mail")
    } else if assessment.is_some_and(|assessment| assessment.queue_latency) {
        Some("delivery_queue_latency")
    } else if assessment.is_some_and(|assessment| assessment.pending) {
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
    policy: Option<&OperationsAlertPolicy>,
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(chrono::Utc::now());
    let assessment = policy.map(|policy| assess_recovery(&snapshot, policy));
    let degraded_reason = if assessment.is_some_and(|assessment| assessment.manual_reconcile) {
        Some("manual_reconcile")
    } else if assessment.is_some_and(|assessment| assessment.failed_action) {
        Some("recovery_action_failed")
    } else if assessment.is_some_and(|assessment| assessment.payment_overdue) {
        Some("payment_overdue")
    } else if assessment.is_some_and(|assessment| assessment.payment_mail_unsent) {
        Some("payment_mail_unsent")
    } else if assessment.is_some_and(|assessment| assessment.relist_pending) {
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

/// 资产对账与偿付能力 section；报告缺失时明确 unknown，不用空数据冒充无差异。
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use auction_application::operations::OperationsAlertRecoverySnapshot;

    use super::{
        OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
        map_delivery, map_fulfillment, map_recovery,
    };

    fn policy() -> OperationsAlertPolicy {
        OperationsAlertPolicy {
            source_timeout: Duration::from_secs(5),
            batch_window: Duration::from_secs(120),
            cooldown: Duration::from_secs(1_800),
            claim_lease: Duration::from_secs(120),
            retry_delay: Duration::from_secs(300),
            max_signals_per_scan: 64,
            max_batch_items: 20,
            max_body_bytes: 4_096,
            delivery_pending_count_threshold: 1,
            delivery_pending_age_threshold: Duration::from_secs(900),
            delivery_manual_reconcile_count_threshold: 1,
            fulfillment_backlog_count_threshold: 1,
            fulfillment_backlog_age_threshold: Duration::from_secs(900),
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_threshold: Duration::from_secs(900),
        }
    }

    fn fulfillment(
        pending: i64,
        waiting_payment: i64,
        waiting_prerequisite: i64,
        age: i64,
        prerequisite_age: i64,
    ) -> OperationsAlertFulfillmentSnapshot {
        OperationsAlertFulfillmentSnapshot {
            pending,
            blocked: 0,
            failed: 0,
            manual_reconcile: 0,
            waiting_payment,
            waiting_prerequisite,
            oldest_backlog_age_seconds: age,
            oldest_prerequisite_wait_age_seconds: prerequisite_age,
        }
    }

    #[test]
    fn fulfillment_uses_shared_scanner_thresholds_and_ignores_payment_waiting() {
        let policy = policy();
        let count_reached = map_fulfillment(fulfillment(4, 4, 0, 10, 0), Some(&policy));
        assert_eq!(count_reached.status, "degraded");
        let age_reached = map_fulfillment(fulfillment(0, 0, 0, 900, 0), Some(&policy));
        assert_eq!(age_reached.status, "degraded");
        let waiting_payment = map_fulfillment(fulfillment(0, 100, 0, 0, 0), Some(&policy));
        assert_eq!(waiting_payment.status, "ok");
        let prerequisite = map_fulfillment(fulfillment(0, 0, 1, 0, 900), Some(&policy));
        assert_eq!(prerequisite.status, "degraded");
    }

    #[test]
    fn delivery_and_recovery_use_the_same_scanner_thresholds() {
        let policy = policy();
        let fresh_delivery = map_delivery(
            OperationsAlertDeliverySnapshot {
                pending: 5,
                failed: 0,
                manual_reconcile: 0,
                returned_mail_manual_reconcile: 0,
                oldest_pending_age_seconds: 10,
                oldest_manual_reconcile_age_seconds: 0,
                oldest_returned_mail_age_seconds: 0,
                claimed: 0,
                oldest_claimed_age_seconds: 0,
                last_update_at: None,
            },
            Some(&policy),
        );
        assert_eq!(fresh_delivery.status, "degraded");
        let old_delivery = map_delivery(
            OperationsAlertDeliverySnapshot {
                pending: 5,
                failed: 0,
                manual_reconcile: 0,
                returned_mail_manual_reconcile: 0,
                oldest_pending_age_seconds: 900,
                oldest_manual_reconcile_age_seconds: 0,
                oldest_returned_mail_age_seconds: 0,
                claimed: 0,
                oldest_claimed_age_seconds: 0,
                last_update_at: None,
            },
            Some(&policy),
        );
        assert_eq!(old_delivery.status, "degraded");
        let fresh_recovery = map_recovery(
            OperationsAlertRecoverySnapshot {
                payment_overdue_count: 4,
                payment_mail_unsent_count: 0,
                relist_pending_count: 0,
                failed_action_count: 0,
                manual_reconcile_count: 0,
                oldest_overdue_age_seconds: 10,
                oldest_payment_mail_unsent_age_seconds: 0,
                worlds: Vec::new(),
            },
            Some(&policy),
        );
        assert_eq!(fresh_recovery.status, "degraded");
        let relist_only = map_recovery(
            OperationsAlertRecoverySnapshot {
                payment_overdue_count: 0,
                payment_mail_unsent_count: 0,
                relist_pending_count: 1,
                failed_action_count: 0,
                manual_reconcile_count: 0,
                oldest_overdue_age_seconds: 0,
                oldest_payment_mail_unsent_age_seconds: 0,
                worlds: Vec::new(),
            },
            Some(&policy),
        );
        assert_eq!(relist_only.status, "degraded");
    }
}
