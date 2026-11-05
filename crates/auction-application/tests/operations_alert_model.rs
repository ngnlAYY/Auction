#![allow(clippy::expect_used, clippy::unwrap_used)]

//! W5.2 告警分级、来源闭集与去重：只验证纯应用层映射与身份绑定，
//! 不连接数据库、不发送通知，也不引入迁移。

use std::collections::BTreeSet;
use std::time::Duration;

use auction_application::{
    OPERATIONS_ALERT_DIGEST_INTERVAL, OperationsAlertError, OperationsAlertKind,
    OperationsAlertSeverity, OperationsAlertSignal, OperationsAlertSource,
    OperationsAlertSourceObservation,
};

const COOLDOWN: Duration = Duration::from_secs(1_800);

/// 每个种类唯一合法的来源与范围；`source_unavailable` 的 scope 等于来源名称。
fn canonical_identity(kind: OperationsAlertKind) -> (OperationsAlertSource, &'static str) {
    use OperationsAlertKind::{
        AuditFailure, BalanceLow, DeliveryManualReconcile, DeliveryPending, DeliveryReturnedMail,
        EvidenceStale, FulfillmentBacklog, FulfillmentFailed, FulfillmentManualReconcile,
        RecoveryFailed, RecoveryManualReconcile, RecoveryPaymentOverdue, RecoveryRelistPending,
        ServiceDegraded, ServiceMissing, SourceUnavailable,
    };
    use OperationsAlertSource::{Delivery, Fulfillment, Process, Recovery};
    match kind {
        ServiceMissing | ServiceDegraded => (Process, "worker"),
        AuditFailure => (Process, "audit"),
        SourceUnavailable => (Process, "process"),
        DeliveryPending | DeliveryReturnedMail | DeliveryManualReconcile => (Delivery, "delivery"),
        FulfillmentBacklog | FulfillmentFailed | FulfillmentManualReconcile => {
            (Fulfillment, "auction")
        }
        RecoveryPaymentOverdue
        | RecoveryRelistPending
        | RecoveryFailed
        | RecoveryManualReconcile => (Recovery, "auction"),
        BalanceLow | EvidenceStale => (Recovery, "main"),
    }
}

fn signal(kind: OperationsAlertKind) -> OperationsAlertSignal {
    let (source, scope) = canonical_identity(kind);
    OperationsAlertSignal::new(kind, scope, source, 1, 0).expect("canonical signal 必须有效")
}

#[test]
fn severity_partitions_the_closed_kind_set() {
    let mut digest = BTreeSet::new();
    for kind in OperationsAlertKind::ALL {
        match kind.severity() {
            OperationsAlertSeverity::Digest => {
                digest.insert(kind.as_str());
            }
            OperationsAlertSeverity::Urgent => {
                assert!(
                    !matches!(
                        kind,
                        OperationsAlertKind::RecoveryPaymentOverdue
                            | OperationsAlertKind::RecoveryRelistPending
                    ),
                    "{} 不应是紧急信号",
                    kind.as_str()
                );
            }
        }
    }
    assert_eq!(digest, BTreeSet::from(["recovery_payment_overdue", "recovery_relist_pending"]));
    assert_eq!(OperationsAlertSeverity::Urgent.as_str(), "urgent");
    assert_eq!(OperationsAlertSeverity::Digest.as_str(), "digest");
    assert_eq!(OperationsAlertSeverity::Digest.to_string(), "digest");
}

#[test]
fn repeat_interval_follows_severity_without_shortening_policy() {
    let urgent = signal(OperationsAlertKind::ServiceMissing);
    assert_eq!(urgent.severity(), OperationsAlertSeverity::Urgent);
    assert_eq!(urgent.repeat_interval(COOLDOWN), COOLDOWN);

    let digest = signal(OperationsAlertKind::RecoveryPaymentOverdue);
    assert_eq!(digest.severity(), OperationsAlertSeverity::Digest);
    assert_eq!(digest.repeat_interval(COOLDOWN), OPERATIONS_ALERT_DIGEST_INTERVAL);

    // 策略冷却本身长于每日节奏时保留策略值，不能因分级缩短已批准的冷却。
    let long_cooldown = OPERATIONS_ALERT_DIGEST_INTERVAL * 2;
    assert_eq!(digest.repeat_interval(long_cooldown), long_cooldown);
    assert_eq!(urgent.repeat_interval(long_cooldown), long_cooldown);
    assert_eq!(OPERATIONS_ALERT_DIGEST_INTERVAL, Duration::from_secs(24 * 3_600));
}

#[test]
fn source_closed_set_round_trips_and_rejects_unknown_names() {
    for source in OperationsAlertSource::ALL {
        assert_eq!(OperationsAlertSource::parse(source.as_str()), Some(source));
        assert_eq!(source.to_string(), source.as_str());
    }
    assert_eq!(OperationsAlertSource::parse(""), None);
    assert_eq!(OperationsAlertSource::parse("backup"), None);
    assert_eq!(OperationsAlertSource::parse("Process"), None);
}

#[test]
fn every_kind_binds_exactly_one_source_and_scope() {
    for kind in OperationsAlertKind::ALL {
        let (source, scope) = canonical_identity(kind);
        assert!(signal(kind).validate().is_ok(), "{} 的规范身份必须有效", kind.as_str());
        for other in OperationsAlertSource::ALL {
            if other == source {
                continue;
            }
            let candidate = OperationsAlertSignal::new(kind, scope, other, 1, 0);
            assert_eq!(
                candidate,
                Err(OperationsAlertError::Invalid),
                "{} 不得绑定来源 {}",
                kind.as_str(),
                other.as_str()
            );
        }
    }
}

#[test]
fn observation_rejects_duplicate_fingerprints() {
    let duplicate = signal(OperationsAlertKind::DeliveryPending);
    let result = OperationsAlertSourceObservation::available(
        OperationsAlertSource::Delivery,
        vec![duplicate.clone(), duplicate],
    );
    assert_eq!(result, Err(OperationsAlertError::Invalid));
}

#[test]
fn observation_keeps_distinct_scopes_of_the_same_kind() {
    let missing = |scope: &str| {
        OperationsAlertSignal::new(
            OperationsAlertKind::ServiceMissing,
            scope,
            OperationsAlertSource::Process,
            1,
            0,
        )
        .expect("进程缺失 signal 必须有效")
    };
    let observation = OperationsAlertSourceObservation::available(
        OperationsAlertSource::Process,
        vec![missing("public"), missing("worker")],
    )
    .expect("不同 scope 必须同时保留");
    assert_eq!(observation.signals.len(), 2);
    assert_ne!(observation.signals[0].fingerprint, observation.signals[1].fingerprint);
}
