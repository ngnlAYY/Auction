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
    /// 有寄出时长达到长期未领取阈值的等待领取（超期件数大于 0）；阈值随快照读取给出，全部回落
    /// 到阈值以下或收口即为 false。
    pub collection_overdue: bool,
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
            || threshold_reached(
                snapshot.oldest_pending_age_seconds,
                policy.delivery_pending_age_threshold,
            ),
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
        && threshold_reached(
            snapshot.oldest_prerequisite_wait_age_seconds,
            policy.fulfillment_backlog_age_threshold,
        );
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
            || threshold_reached(oldest_backlog_age, policy.fulfillment_backlog_age_threshold),
        failed: snapshot.failed > 0,
        manual_reconcile: snapshot.manual_reconcile > 0,
        collection_overdue: snapshot.collection_overdue > 0,
    }
}

pub fn assess_recovery(
    snapshot: &OperationsAlertRecoverySnapshot,
    policy: &OperationsAlertPolicy,
) -> RecoveryAssessment {
    RecoveryAssessment {
        payment_overdue: snapshot.payment_overdue_count >= policy.recovery_overdue_count_threshold
            || threshold_reached(
                snapshot.oldest_overdue_age_seconds,
                policy.recovery_overdue_age_threshold,
            ),
        payment_mail_unsent: snapshot.payment_mail_unsent_count > 0,
        relist_pending: snapshot.relist_pending_count > 0,
        failed_action: snapshot.failed_action_count > 0,
        manual_reconcile: snapshot.manual_reconcile_count > 0,
    }
}

fn threshold_reached(age_seconds: i64, threshold: Duration) -> bool {
    u64::try_from(age_seconds).is_ok_and(|age| age >= threshold.as_secs())
}

fn reached(age_seconds: i64, threshold: Duration) -> bool {
    threshold_reached(age_seconds, threshold)
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
        OperationsAlertReturnTakeUnknown,
    };
    use crate::ports::operations::alert::signals::{
        OperationsAlertError, OperationsAlertKind, OperationsAlertSource,
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
                awaiting_collection: 0,
                awaiting_collection_unreadable: 0,
                oldest_backlog_age_seconds: 10,
                oldest_prerequisite_wait_age_seconds: 0,
                oldest_awaiting_collection_age_seconds: 0,
                oldest_awaiting_collection_backlog_age_seconds: 0,
                collection_overdue: 0,
                settlement_return_pending: 0,
                oldest_settlement_return_pending_age_seconds: 0,
                settlement_return_quarantined: 0,
                oldest_settlement_return_quarantined_age_seconds: 0,
                settlement_return_take_unknown: Default::default(),
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
            awaiting_collection: 0,
            awaiting_collection_unreadable: 0,
            oldest_backlog_age_seconds: 10,
            oldest_prerequisite_wait_age_seconds: 0,
            oldest_awaiting_collection_age_seconds: 0,
            oldest_awaiting_collection_backlog_age_seconds: 0,
            collection_overdue: 0,
            settlement_return_pending: 0,
            oldest_settlement_return_pending_age_seconds: 0,
            settlement_return_quarantined: 0,
            oldest_settlement_return_quarantined_age_seconds: 0,
            settlement_return_take_unknown: Default::default(),
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
            updated_at: auction_time::Timestamp::now(),
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
            awaiting_collection: 0,
            awaiting_collection_unreadable: 0,
            oldest_backlog_age_seconds: 0,
            oldest_prerequisite_wait_age_seconds: 10,
            oldest_awaiting_collection_age_seconds: 0,
            oldest_awaiting_collection_backlog_age_seconds: 0,
            collection_overdue: 0,
            settlement_return_pending: 0,
            oldest_settlement_return_pending_age_seconds: 0,
            settlement_return_quarantined: 0,
            oldest_settlement_return_quarantined_age_seconds: 0,
            settlement_return_take_unknown: Default::default(),
        };
        assert!(!assess_fulfillment(snapshot, &policy).backlog);
        let snapshot = OperationsAlertFulfillmentSnapshot {
            oldest_prerequisite_wait_age_seconds: 900,
            ..snapshot
        };
        assert!(assess_fulfillment(snapshot, &policy).backlog);
    }

    /// 已寄出、只等收件人领取的义务单列为等待领取：即使数量多、已等待数天也不触发积压；
    /// 读取失败数只能是等待领取的子集，等待年龄只能随等待领取出现。
    #[test]
    fn awaiting_collection_is_not_backlog_and_must_be_consistent() {
        let policy = policy();
        let awaiting = OperationsAlertFulfillmentSnapshot {
            pending: 0,
            blocked: 0,
            failed: 0,
            manual_reconcile: 0,
            waiting_payment: 0,
            waiting_prerequisite: 0,
            awaiting_collection: 36,
            awaiting_collection_unreadable: 36,
            oldest_backlog_age_seconds: 0,
            oldest_prerequisite_wait_age_seconds: 0,
            oldest_awaiting_collection_age_seconds: 8 * 86_400,
            oldest_awaiting_collection_backlog_age_seconds: 0,
            collection_overdue: 0,
            settlement_return_pending: 0,
            oldest_settlement_return_pending_age_seconds: 0,
            settlement_return_quarantined: 0,
            oldest_settlement_return_quarantined_age_seconds: 0,
            settlement_return_take_unknown: Default::default(),
        };
        assert!(!assess_fulfillment(awaiting, &policy).backlog, "等待领取不是待处理积压");
        let observations = evaluate_snapshot(
            &OperationsAlertSnapshot {
                fulfillment: Some(awaiting),
                ..OperationsAlertSnapshot::default()
            },
            &policy,
        )
        .expect("一致的等待领取水位必须可评估");
        let fulfillment = observations
            .iter()
            .find(|observation| observation.source == OperationsAlertSource::Fulfillment)
            .expect("履约来源");
        assert!(
            !fulfillment
                .signals
                .iter()
                .any(|signal| signal.kind == OperationsAlertKind::FulfillmentBacklog),
            "等待领取不得产生积压告警"
        );
        for broken in [
            OperationsAlertFulfillmentSnapshot { awaiting_collection_unreadable: 37, ..awaiting },
            OperationsAlertFulfillmentSnapshot {
                awaiting_collection: 0,
                awaiting_collection_unreadable: 0,
                ..awaiting
            },
            OperationsAlertFulfillmentSnapshot {
                oldest_awaiting_collection_age_seconds: -1,
                oldest_awaiting_collection_backlog_age_seconds: 0,
                ..awaiting
            },
            OperationsAlertFulfillmentSnapshot { awaiting_collection_unreadable: -1, ..awaiting },
            // 积压时刻口径的等待领取年龄同样不得为负，没有等待领取时只能为 0。
            OperationsAlertFulfillmentSnapshot {
                oldest_awaiting_collection_backlog_age_seconds: -1,
                ..awaiting
            },
            OperationsAlertFulfillmentSnapshot {
                awaiting_collection: 0,
                awaiting_collection_unreadable: 0,
                oldest_awaiting_collection_age_seconds: 0,
                oldest_awaiting_collection_backlog_age_seconds: 60,
                ..awaiting
            },
        ] {
            let snapshot = OperationsAlertSnapshot {
                fulfillment: Some(broken),
                ..OperationsAlertSnapshot::default()
            };
            assert_eq!(evaluate_snapshot(&snapshot, &policy), Err(OperationsAlertError::Invalid));
        }
    }

    /// 长期未领取：信号计数是达到阈值的件数（超期件数），年龄是最久寄出时长；没有超期件时不
    /// 告警（总等待数多、最久一笔仍在阈值以内也一样），超期件全部回落或收口后解除。超期件数
    /// 不能多于等待领取，有超期件时最久一笔必须达到本策略阈值，否则快照按别的阈值读取，失败
    /// 关闭。扫描结果与看板判定一致。
    #[test]
    fn collection_overdue_counts_items_past_threshold_and_resolves() {
        let policy = policy();
        let threshold = policy.fulfillment_collection_overdue_age_threshold.as_secs() as i64;
        let awaiting = |count: i64, overdue: i64, age: i64| OperationsAlertFulfillmentSnapshot {
            pending: 0,
            blocked: 0,
            failed: 0,
            manual_reconcile: 0,
            waiting_payment: 0,
            waiting_prerequisite: 0,
            awaiting_collection: count,
            awaiting_collection_unreadable: 0,
            oldest_backlog_age_seconds: 0,
            oldest_prerequisite_wait_age_seconds: 0,
            oldest_awaiting_collection_age_seconds: age,
            oldest_awaiting_collection_backlog_age_seconds: 0,
            collection_overdue: overdue,
            settlement_return_pending: 0,
            oldest_settlement_return_pending_age_seconds: 0,
            settlement_return_quarantined: 0,
            oldest_settlement_return_quarantined_age_seconds: 0,
            settlement_return_take_unknown: Default::default(),
        };
        let evaluate = |snapshot: OperationsAlertFulfillmentSnapshot| {
            let snapshot = OperationsAlertSnapshot {
                fulfillment: Some(snapshot),
                ..OperationsAlertSnapshot::default()
            };
            evaluate_snapshot(&snapshot, &policy)
        };
        let overdue_signal = |snapshot: OperationsAlertFulfillmentSnapshot| {
            evaluate(snapshot)
                .expect("等待领取水位必须可评估")
                .into_iter()
                .find(|observation| observation.source == OperationsAlertSource::Fulfillment)
                .and_then(|observation| {
                    observation.signals.into_iter().find(|signal| {
                        signal.kind == OperationsAlertKind::FulfillmentCollectionOverdue
                    })
                })
                .map(|signal| (signal.count, signal.age_seconds))
        };
        // 100 笔等待领取里只有 1 笔超期：信号与看板都只计这 1 笔，总数只是背景。
        let one_overdue = awaiting(100, 1, threshold + 86_400);
        assert_eq!(overdue_signal(one_overdue), Some((1, threshold + 86_400)));
        assert!(assess_fulfillment(one_overdue, &policy).collection_overdue);
        assert!(!assess_fulfillment(one_overdue, &policy).backlog, "长期未领取不是积压");
        let at_threshold = awaiting(36, 36, threshold);
        assert_eq!(overdue_signal(at_threshold), Some((36, threshold)));
        let within = awaiting(36, 0, threshold - 1);
        assert_eq!(overdue_signal(within), None);
        assert!(!assess_fulfillment(within, &policy).collection_overdue);
        let collected = awaiting(0, 0, 0);
        assert_eq!(overdue_signal(collected), None, "全部收口后告警解除");
        assert!(!assess_fulfillment(collected, &policy).collection_overdue);
        for broken in [
            awaiting(3, 4, threshold),
            awaiting(3, -1, threshold),
            awaiting(3, 1, threshold - 1),
            awaiting(0, 1, 0),
        ] {
            assert_eq!(evaluate(broken), Err(OperationsAlertError::Invalid), "{broken:?}");
        }
    }

    /// Take 结果未知的退件按原因各出一个信号（作用域即原因），只计未知案件；隔离信号不再包含
    /// 结果未知。原因水位为负、或没有案件却有年龄时失败关闭。
    #[test]
    fn take_unknown_returns_signal_per_reason() {
        let policy = policy();
        let mut snapshot = OperationsAlertFulfillmentSnapshot {
            pending: 0,
            blocked: 0,
            failed: 0,
            manual_reconcile: 0,
            waiting_payment: 0,
            waiting_prerequisite: 0,
            awaiting_collection: 0,
            awaiting_collection_unreadable: 0,
            oldest_backlog_age_seconds: 0,
            oldest_prerequisite_wait_age_seconds: 0,
            oldest_awaiting_collection_age_seconds: 0,
            oldest_awaiting_collection_backlog_age_seconds: 0,
            collection_overdue: 0,
            settlement_return_pending: 0,
            oldest_settlement_return_pending_age_seconds: 0,
            settlement_return_quarantined: 1,
            oldest_settlement_return_quarantined_age_seconds: 60,
            settlement_return_take_unknown: Default::default(),
        };
        let mail_missing = auction_domain::SettlementReturnTakeUnknownReason::ALL
            .iter()
            .position(|reason| reason.as_str() == "mail_missing")
            .expect("原因闭集包含 mail_missing");
        snapshot.settlement_return_take_unknown[0] =
            OperationsAlertReturnTakeUnknown { count: 1, oldest_age_seconds: 30 };
        snapshot.settlement_return_take_unknown[mail_missing] =
            OperationsAlertReturnTakeUnknown { count: 2, oldest_age_seconds: 900 };
        let evaluate = |snapshot: OperationsAlertFulfillmentSnapshot| {
            evaluate_snapshot(
                &OperationsAlertSnapshot {
                    fulfillment: Some(snapshot),
                    ..OperationsAlertSnapshot::default()
                },
                &policy,
            )
        };
        let signals = evaluate(snapshot)
            .expect("退件水位必须可评估")
            .into_iter()
            .find(|observation| observation.source == OperationsAlertSource::Fulfillment)
            .map_or_default(|observation| observation.signals)
            .into_iter()
            .map(|signal| (signal.kind, signal.scope, signal.count, signal.age_seconds))
            .collect::<Vec<_>>();
        assert_eq!(
            signals,
            vec![
                (OperationsAlertKind::SettlementReturnQuarantined, "auction".to_owned(), 1, 60),
                (
                    OperationsAlertKind::SettlementReturnTakeUnknown,
                    "assets_claimable".to_owned(),
                    1,
                    30
                ),
                (
                    OperationsAlertKind::SettlementReturnTakeUnknown,
                    "mail_missing".to_owned(),
                    2,
                    900
                ),
            ]
        );
        let mut broken = snapshot;
        broken.settlement_return_take_unknown[mail_missing].count = -1;
        assert_eq!(evaluate(broken), Err(OperationsAlertError::Invalid));
        let mut aged = snapshot;
        aged.settlement_return_take_unknown[mail_missing] =
            OperationsAlertReturnTakeUnknown { count: 0, oldest_age_seconds: 5 };
        assert_eq!(evaluate(aged), Err(OperationsAlertError::Invalid));
    }
}
