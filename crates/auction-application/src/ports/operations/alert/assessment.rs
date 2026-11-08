//! Dashboard 与告警扫描器共享的阈值判定；只返回布尔分类，不生成 signal 或改业务状态。

use std::time::Duration;

use super::model::{
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
    OperationsAlertRecoverySnapshot,
};
use super::signals::OPERATIONS_ALERT_QUEUE_LATENCY_AGE;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeliveryAssessment {
    pub queue_latency: bool,
    pub pending: bool,
    pub returned_mail: bool,
    pub manual_reconcile: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FulfillmentAssessment {
    pub backlog: bool,
    pub failed: bool,
    pub manual_reconcile: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryAssessment {
    pub payment_overdue: bool,
    pub payment_mail_unsent: bool,
    pub relist_pending: bool,
    pub failed_action: bool,
    pub manual_reconcile: bool,
}

pub fn assess_delivery(
    snapshot: OperationsAlertDeliverySnapshot,
    policy: &OperationsAlertPolicy,
) -> DeliveryAssessment {
    DeliveryAssessment {
        queue_latency: snapshot.claimed > 0
            && reached(snapshot.oldest_claimed_age_seconds, OPERATIONS_ALERT_QUEUE_LATENCY_AGE),
        pending: snapshot.pending >= policy.delivery_pending_count_threshold
            || snapshot.oldest_pending_age_seconds
                >= policy.delivery_pending_age_threshold.as_secs() as i64,
        returned_mail: snapshot.returned_mail_manual_reconcile
            >= policy.delivery_manual_reconcile_count_threshold,
        manual_reconcile: snapshot.manual_reconcile
            >= policy.delivery_manual_reconcile_count_threshold,
    }
}

pub fn assess_fulfillment(
    snapshot: OperationsAlertFulfillmentSnapshot,
    policy: &OperationsAlertPolicy,
) -> FulfillmentAssessment {
    let actionable = snapshot.pending.saturating_add(snapshot.blocked);
    let waiting_overdue = snapshot.waiting_prerequisite
        >= policy.fulfillment_backlog_count_threshold
        && snapshot.oldest_prerequisite_wait_age_seconds
            >= policy.fulfillment_backlog_age_threshold.as_secs() as i64;
    let backlog = if waiting_overdue {
        actionable.saturating_add(snapshot.waiting_prerequisite)
    } else {
        actionable
    };
    let oldest_backlog_age = if waiting_overdue {
        snapshot.oldest_backlog_age_seconds.max(snapshot.oldest_prerequisite_wait_age_seconds)
    } else {
        snapshot.oldest_backlog_age_seconds
    };
    FulfillmentAssessment {
        backlog: backlog >= policy.fulfillment_backlog_count_threshold
            || oldest_backlog_age >= policy.fulfillment_backlog_age_threshold.as_secs() as i64,
        failed: snapshot.failed > 0,
        manual_reconcile: snapshot.manual_reconcile > 0,
    }
}

pub fn assess_recovery(
    snapshot: &OperationsAlertRecoverySnapshot,
    policy: &OperationsAlertPolicy,
) -> RecoveryAssessment {
    RecoveryAssessment {
        payment_overdue: snapshot.payment_overdue_count >= policy.recovery_overdue_count_threshold
            || snapshot.oldest_overdue_age_seconds
                >= policy.recovery_overdue_age_threshold.as_secs() as i64,
        payment_mail_unsent: snapshot.payment_mail_unsent_count > 0,
        relist_pending: snapshot.relist_pending_count > 0,
        failed_action: snapshot.failed_action_count > 0,
        manual_reconcile: snapshot.manual_reconcile_count > 0,
    }
}

fn reached(age_seconds: i64, threshold: Duration) -> bool {
    age_seconds >= i64::try_from(threshold.as_secs()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{assess_delivery, assess_fulfillment, assess_recovery};
    use crate::ports::operations::alert::evaluate_snapshot;
    use crate::ports::operations::alert::model::OperationsAlertSnapshot;
    use crate::ports::operations::alert::model::{
        OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertPolicy,
        OperationsAlertRecoverySnapshot, OperationsAlertRecoveryWorld,
    };
    use crate::ports::operations::alert::signals::{OperationsAlertKind, OperationsAlertSource};

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

    #[test]
    fn dashboard_assessment_matches_scanner_or_semantics() {
        let policy = policy();
        let delivery = assess_delivery(
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
            &policy,
        );
        assert!(delivery.pending, "count 达标即应告警，不要求 age 同时达标");
        let fulfillment = assess_fulfillment(
            OperationsAlertFulfillmentSnapshot {
                pending: 5,
                blocked: 0,
                failed: 0,
                manual_reconcile: 0,
                waiting_payment: 0,
                waiting_prerequisite: 0,
                oldest_backlog_age_seconds: 10,
                oldest_prerequisite_wait_age_seconds: 0,
            },
            &policy,
        );
        assert!(fulfillment.backlog);
        let mut recovery = OperationsAlertRecoverySnapshot {
            payment_overdue_count: 0,
            payment_mail_unsent_count: 0,
            relist_pending_count: 1,
            failed_action_count: 0,
            manual_reconcile_count: 0,
            oldest_overdue_age_seconds: 0,
            oldest_payment_mail_unsent_age_seconds: 0,
            worlds: Vec::new(),
        };
        assert!(assess_recovery(&recovery, &policy).relist_pending);
        recovery.relist_pending_count = 0;
        assert!(!assess_recovery(&recovery, &policy).relist_pending);
    }

    #[test]
    fn scanner_signal_presence_matches_dashboard_assessment() {
        let policy = policy();
        let snapshot = OperationsAlertSnapshot {
            delivery: Some(OperationsAlertDeliverySnapshot {
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
            }),
            ..OperationsAlertSnapshot::default()
        };
        let observations = evaluate_snapshot(&snapshot, &policy).expect("扫描评估");
        let delivery = observations
            .iter()
            .find(|observation| observation.source == OperationsAlertSource::Delivery)
            .expect("交付来源");
        let scanner_pending = delivery
            .signals
            .iter()
            .any(|signal| signal.kind == OperationsAlertKind::DeliveryPending);
        let dashboard = assess_delivery(snapshot.delivery.expect("交付快照"), &policy);
        assert_eq!(scanner_pending, dashboard.pending);
    }

    #[test]
    fn scanner_signal_presence_matches_dashboard_for_fulfillment_and_recovery() {
        let policy = policy();
        let fulfillment = OperationsAlertFulfillmentSnapshot {
            pending: 5,
            blocked: 0,
            failed: 0,
            manual_reconcile: 0,
            waiting_payment: 0,
            waiting_prerequisite: 0,
            oldest_backlog_age_seconds: 10,
            oldest_prerequisite_wait_age_seconds: 0,
        };
        let recovery_world = |world: &str| OperationsAlertRecoveryWorld {
            world: world.to_owned(),
            balance_credits: 0,
            balance_status: "ok".to_owned(),
            evidence_status: "fresh".to_owned(),
            evidence_age_seconds: 0,
            balance_stall_age_seconds: 0,
            balance_observed_at: None,
            evidence_last_success_at: None,
            updated_at: chrono::Utc::now(),
        };
        let recovery = OperationsAlertRecoverySnapshot {
            payment_overdue_count: 0,
            payment_mail_unsent_count: 0,
            relist_pending_count: 1,
            failed_action_count: 0,
            manual_reconcile_count: 0,
            oldest_overdue_age_seconds: 0,
            oldest_payment_mail_unsent_age_seconds: 0,
            worlds: vec![recovery_world("main"), recovery_world("isekai")],
        };
        let snapshot = OperationsAlertSnapshot {
            fulfillment: Some(fulfillment),
            recovery: Some(recovery.clone()),
            ..OperationsAlertSnapshot::default()
        };
        let observations = evaluate_snapshot(&snapshot, &policy).expect("扫描评估");
        let scanner_fulfillment = observations
            .iter()
            .find(|observation| observation.source == OperationsAlertSource::Fulfillment)
            .expect("履约来源")
            .signals
            .iter()
            .any(|signal| signal.kind == OperationsAlertKind::FulfillmentBacklog);
        assert_eq!(scanner_fulfillment, assess_fulfillment(fulfillment, &policy).backlog);
        let scanner_relist = observations
            .iter()
            .find(|observation| observation.source == OperationsAlertSource::Recovery)
            .expect("恢复来源")
            .signals
            .iter()
            .any(|signal| signal.kind == OperationsAlertKind::RecoveryRelistPending);
        assert_eq!(scanner_relist, assess_recovery(&recovery, &policy).relist_pending);
    }

    #[test]
    fn waiting_prerequisite_requires_both_thresholds() {
        let policy = policy();
        let snapshot = OperationsAlertFulfillmentSnapshot {
            pending: 0,
            blocked: 0,
            failed: 0,
            manual_reconcile: 0,
            waiting_payment: 0,
            waiting_prerequisite: 5,
            oldest_backlog_age_seconds: 0,
            oldest_prerequisite_wait_age_seconds: 10,
        };
        assert!(!assess_fulfillment(snapshot, &policy).backlog);
        let snapshot = OperationsAlertFulfillmentSnapshot {
            oldest_prerequisite_wait_age_seconds: 900,
            ..snapshot
        };
        assert!(assess_fulfillment(snapshot, &policy).backlog);
    }
}
