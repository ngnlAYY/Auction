#![allow(clippy::expect_used, clippy::unwrap_used)]

//! W5.8 告警台投影与命令边界：只验证纯应用层闭集、分页校验和幂等摘要，
//! 不连接数据库、不发送通知。

use std::collections::BTreeSet;

use auction_application::operations::{
    OperationsAlertConsoleEntry, OperationsAlertConsolePage, OperationsAlertConsoleQuery,
    OperationsAlertError, OperationsAlertFilter, OperationsAlertKind,
    OperationsAlertSilenceCommand, OperationsAlertSource, parse_alert_id,
};
use chrono::{Duration, Utc};
use uuid::Uuid;

#[test]
fn every_alert_kind_has_a_unique_runbook_anchor_per_source() {
    let mut anchors = BTreeSet::new();
    for kind in OperationsAlertKind::ALL {
        assert_eq!(OperationsAlertKind::parse(kind.as_str()), Some(kind));
    }
    for source in OperationsAlertSource::ALL {
        assert_eq!(OperationsAlertSource::parse(source.as_str()), Some(source));
        assert!(
            anchors.insert(source.runbook_anchor()),
            "source {source} repeats a runbook anchor"
        );
        assert!(source.runbook_anchor().starts_with("alert-runbook-"));
    }
    assert!(OperationsAlertKind::parse("unknown_kind").is_none());
    assert!(OperationsAlertSource::parse("unknown_source").is_none());
}

#[test]
fn filter_and_pagination_are_fail_closed() {
    assert_eq!(OperationsAlertFilter::parse("active"), Some(OperationsAlertFilter::Active));
    assert_eq!(OperationsAlertFilter::parse("resolved"), Some(OperationsAlertFilter::Resolved));
    assert_eq!(OperationsAlertFilter::parse("all"), Some(OperationsAlertFilter::All));
    assert_eq!(OperationsAlertFilter::parse(""), None);
    assert_eq!(OperationsAlertFilter::default(), OperationsAlertFilter::Active);

    let query = OperationsAlertConsoleQuery {
        filter: OperationsAlertFilter::Active,
        source: Some(OperationsAlertSource::Delivery),
        page: 2,
        page_size: 25,
    };
    assert_eq!(query.validate(), Ok(()));
    assert_eq!(query.offset(), Ok(25));

    let entry = OperationsAlertConsoleEntry {
        fingerprint: [7; 32],
        kind: OperationsAlertKind::DeliveryManualReconcile,
        scope: "delivery".to_owned(),
        source: OperationsAlertSource::Delivery,
        active: true,
        count: 2,
        age_seconds: 90,
        first_seen_at: Utc::now() - Duration::minutes(5),
        last_seen_at: Utc::now(),
        resolved_at: None,
        last_notified_at: None,
        next_notify_at: Some(Utc::now() + Duration::minutes(30)),
        acknowledged_at: None,
        acknowledged_by: None,
        silenced_until: None,
    };
    assert_eq!(entry.validate(), Ok(()));
    assert_eq!(entry.runbook_anchor(), "alert-runbook-delivery");
    assert_eq!(parse_alert_id(&entry.alert_id()), Some(entry.fingerprint));

    let page = OperationsAlertConsolePage {
        entries: vec![entry.clone()],
        page: 1,
        page_size: 25,
        total: 1,
        active_total: 1,
        resolved_total: 0,
    };
    // 响应页必须与请求身份一致；错页不能静默展示。
    assert_eq!(page.validate(&query), Err(OperationsAlertError::StoredResultCorrupt));

    // 活动周期与恢复时间必须互斥；伪造的历史条目失败关闭。
    let mut inconsistent = entry;
    inconsistent.active = false;
    inconsistent.resolved_at = None;
    assert_eq!(inconsistent.validate(), Err(OperationsAlertError::StoredResultCorrupt));
}

#[test]
fn silence_hash_binds_action_duration_actor_and_reason() {
    let fingerprint = [3; 32];
    let base = OperationsAlertSilenceCommand::request_hash(&fingerprint, 3_600, "admin", "note");
    assert_eq!(
        base,
        OperationsAlertSilenceCommand::request_hash(&fingerprint, 3_600, "admin", "note")
    );
    for other in [
        OperationsAlertSilenceCommand::request_hash(&fingerprint, 7_200, "admin", "note"),
        OperationsAlertSilenceCommand::request_hash(&fingerprint, 3_600, "other", "note"),
        OperationsAlertSilenceCommand::request_hash(&fingerprint, 3_600, "admin", "other"),
    ] {
        assert_ne!(base, other);
    }

    assert!(matches!(
        OperationsAlertSilenceCommand::new(Uuid::new_v4(), [0; 32], 3_600, "admin", ""),
        Err(OperationsAlertError::Invalid)
    ));
    assert!(matches!(
        OperationsAlertSilenceCommand::new(Uuid::nil(), fingerprint, 3_600, "admin", ""),
        Err(OperationsAlertError::Invalid)
    ));
}
