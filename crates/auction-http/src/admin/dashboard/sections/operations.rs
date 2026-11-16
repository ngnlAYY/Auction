//! Admin dashboard 履约、投递和 recovery 水位 section。

use auction_application::operations::{
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
    OperationsAlertReconciliationSnapshot, OperationsAlertRecoverySnapshot, assess_delivery,
    assess_fulfillment, assess_recovery,
};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::json;

use crate::mapping::timestamp;

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
    } else if assessment.is_some_and(|assessment| assessment.collection_overdue) {
        Some("fulfillment_collection_overdue")
    } else {
        None
    };
    // 长期未领取与告警扫描同一阈值判定；没有告警策略时不声称超期。
    let collection_overdue = assessment.is_some_and(|assessment| assessment.collection_overdue);
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
            "awaiting_collection": snapshot.awaiting_collection,
            "awaiting_collection_unreadable": snapshot.awaiting_collection_unreadable,
            "oldest_backlog_age_seconds": snapshot.oldest_backlog_age_seconds,
            "oldest_prerequisite_wait_age_seconds": snapshot.oldest_prerequisite_wait_age_seconds,
            "oldest_awaiting_collection_age_seconds":
                snapshot.oldest_awaiting_collection_age_seconds,
            "collection_overdue": collection_overdue,
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
/// 在途装备与在途货款只展示，不参与降级判定：它们已随邮件离开 Bot，等待收件人领取。
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
                "custody_in_transit_count": world.custody_in_transit_count,
                "solvency_shortfall_credits": world.solvency_shortfall_credits,
                "solvency_in_transit_count": world.solvency_in_transit_count,
                "solvency_in_transit_credits": world.solvency_in_transit_credits,
            })).collect::<Vec<_>>(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use auction_application::operations::{
        OperationsAlertReconciliationWorld, OperationsAlertRecoverySnapshot,
    };

    use super::{
        OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
        OperationsAlertReconciliationSnapshot, map_delivery, map_fulfillment, map_reconciliation,
        map_recovery,
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
            fulfillment_collection_overdue_age_threshold: Duration::from_secs(259_200),
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
            awaiting_collection: 0,
            awaiting_collection_unreadable: 0,
            oldest_backlog_age_seconds: age,
            oldest_prerequisite_wait_age_seconds: prerequisite_age,
            oldest_awaiting_collection_age_seconds: 0,
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

    /// 等待领取单列为独立分组，计数、读取失败数与最老等待年龄原样投影；只有最久寄出时长达到
    /// 长期未领取阈值时才降级，与告警扫描同一判定。
    #[test]
    fn awaiting_collection_is_projected_separately_without_backlog() {
        let policy = policy();
        let section = map_fulfillment(
            OperationsAlertFulfillmentSnapshot {
                awaiting_collection: 36,
                awaiting_collection_unreadable: 12,
                oldest_awaiting_collection_age_seconds: 691_200,
                ..fulfillment(0, 0, 0, 0, 0)
            },
            Some(&policy),
        );
        assert_eq!(section.status, "degraded", "最久已寄出 8 天，超过 72 小时阈值");
        let data = section.data.expect("履约数据");
        assert_eq!(data["pending"], 0);
        assert_eq!(data["awaiting_collection"], 36);
        assert_eq!(data["awaiting_collection_unreadable"], 12);
        assert_eq!(data["oldest_awaiting_collection_age_seconds"], 691_200);
        assert_eq!(data["collection_overdue"], true);

        // 阈值以内只单列，不降级；没有告警策略时不声称超期。
        let fresh = OperationsAlertFulfillmentSnapshot {
            awaiting_collection: 36,
            oldest_awaiting_collection_age_seconds: 259_199,
            ..fulfillment(0, 0, 0, 0, 0)
        };
        let section = map_fulfillment(fresh, Some(&policy));
        assert_eq!(section.status, "ok");
        assert_eq!(section.data.expect("履约数据")["collection_overdue"], false);
        let without_policy = map_fulfillment(
            OperationsAlertFulfillmentSnapshot {
                oldest_awaiting_collection_age_seconds: 691_200,
                ..fresh
            },
            None,
        );
        assert_eq!(without_policy.status, "ok");
        assert_eq!(without_policy.data.expect("履约数据")["collection_overdue"], false);
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

    /// 在途装备与在途货款原样投影但不使对账分区降级；缺失与缺口仍按差异降级。
    #[test]
    fn reconciliation_in_transit_is_projected_without_degrading() {
        let world = |world, custody_in_transit_count, solvency_in_transit_credits| {
            OperationsAlertReconciliationWorld {
                world,
                custody_missing_count: 0,
                custody_in_transit_count,
                solvency_shortfall_credits: 0,
                solvency_in_transit_count: i64::from(solvency_in_transit_credits > 0),
                solvency_in_transit_credits,
            }
        };
        let snapshot = OperationsAlertReconciliationSnapshot {
            custody_report_age_seconds: Some(60),
            solvency_report_age_seconds: Some(60),
            worlds: vec![world("isekai", 0, 0), world("main", 5, 38_668_000)],
        };
        let section = map_reconciliation(snapshot.clone());
        assert_eq!(section.status, "ok", "在途不是差异");
        let data = section.data.expect("对账数据");
        assert_eq!(data["worlds"][1]["custody_in_transit_count"], 5);
        assert_eq!(data["worlds"][1]["solvency_in_transit_count"], 1);
        assert_eq!(data["worlds"][1]["solvency_in_transit_credits"], 38_668_000);

        let mut drifted = snapshot;
        drifted.worlds[1].custody_missing_count = 1;
        assert_eq!(map_reconciliation(drifted).status, "degraded");
    }
}
