#![allow(clippy::expect_used, clippy::unwrap_used)]

//! W5.2/W5.5 告警分级、来源闭集、主机侧事实映射与去重：只验证纯应用层映射与身份绑定，
//! 不连接数据库、不发送通知，也不引入迁移。

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::Utc;

use auction_application::operations::{
    OPERATIONS_ALERT_DIGEST_INTERVAL, OPERATIONS_ALERT_LIFECYCLE_GRACE,
    OPERATIONS_ALERT_MAIL_POLL_STALL_AGE, OPERATIONS_ALERT_QUEUE_LATENCY_AGE,
    OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE, OperationsAlertCredentialSnapshot,
    OperationsAlertDeliverySnapshot, OperationsAlertError, OperationsAlertForumSnapshot,
    OperationsAlertHostBackupFacts, OperationsAlertHostBackupOutcome, OperationsAlertHostBackupRun,
    OperationsAlertHostDisk, OperationsAlertHostObserveCommand, OperationsAlertHostWalFacts,
    OperationsAlertKind, OperationsAlertLifecycleSnapshot, OperationsAlertMailSnapshot,
    OperationsAlertNotificationSnapshot, OperationsAlertPolicy,
    OperationsAlertReconciliationSnapshot, OperationsAlertReconciliationWorld,
    OperationsAlertRecoverySnapshot, OperationsAlertRecoveryWorld, OperationsAlertSeverity,
    OperationsAlertSignal, OperationsAlertSnapshot, OperationsAlertSource,
    OperationsAlertSourceObservation, evaluate_host_backup, evaluate_host_wal, evaluate_snapshot,
};

const COOLDOWN: Duration = Duration::from_secs(1_800);

/// 每个种类唯一合法的来源与范围；`source_unavailable` 的 scope 等于来源名称。
fn canonical_identity(kind: OperationsAlertKind) -> (OperationsAlertSource, &'static str) {
    use OperationsAlertKind::{
        AccountBusy, AuctionPhaseOverdue, AuditFailure, BackupFailed, BalanceLow,
        CredentialInvalid, CustodyDrift, DeliveryManualReconcile, DeliveryPending,
        DeliveryReturnedMail, DiskCapacityLow, EvidenceStale, ForumMessageFailed, ForumSyncFailed,
        FulfillmentBacklog, FulfillmentFailed, FulfillmentManualReconcile, MailPollStalled,
        NotificationDeliveryFailed, NotificationPostageInsufficient, ProxyExpiryOverdue,
        QueueLatency, ReconciliationStale, RecoveryFailed, RecoveryManualReconcile,
        RecoveryPaymentMailUnsent, RecoveryPaymentOverdue, RecoveryRelistPending, ServiceDegraded,
        ServiceMissing, SolvencyShortfall, SourceUnavailable, WalUploadStalled,
    };
    use OperationsAlertSource::{
        Backup, Credential, Delivery, Forum, Fulfillment, Lifecycle, Mail, Notification, Process,
        Reconciliation, Recovery, Wal,
    };
    match kind {
        ServiceMissing | ServiceDegraded => (Process, "worker"),
        AuditFailure => (Process, "audit"),
        SourceUnavailable => (Process, "process"),
        DeliveryPending | DeliveryReturnedMail | DeliveryManualReconcile | QueueLatency => {
            (Delivery, "delivery")
        }
        FulfillmentBacklog | FulfillmentFailed | FulfillmentManualReconcile => {
            (Fulfillment, "auction")
        }
        RecoveryPaymentOverdue
        | RecoveryPaymentMailUnsent
        | RecoveryRelistPending
        | RecoveryFailed
        | RecoveryManualReconcile => (Recovery, "auction"),
        BalanceLow | EvidenceStale | AccountBusy => (Recovery, "main"),
        MailPollStalled => (Mail, "mail"),
        CredentialInvalid => (Credential, "mail_cookie"),
        AuctionPhaseOverdue | ProxyExpiryOverdue => (Lifecycle, "auction"),
        ForumSyncFailed | ForumMessageFailed => (Forum, "forum"),
        NotificationDeliveryFailed | NotificationPostageInsufficient => {
            (Notification, "notification")
        }
        DiskCapacityLow | BackupFailed => (Backup, "backup"),
        WalUploadStalled => (Wal, "wal"),
        CustodyDrift | SolvencyShortfall => (Reconciliation, "main"),
        ReconciliationStale => (Reconciliation, "custody"),
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
                            | OperationsAlertKind::QueueLatency
                            | OperationsAlertKind::ForumSyncFailed
                            | OperationsAlertKind::NotificationDeliveryFailed
                    ),
                    "{} 不应是紧急信号",
                    kind.as_str()
                );
            }
        }
    }
    // 决策 66：自动重试/自动推进的等待类信号并入每日摘要，人工才能修复的保持紧急。
    assert_eq!(
        digest,
        BTreeSet::from([
            "recovery_payment_overdue",
            "recovery_relist_pending",
            "queue_latency",
            "forum_sync_failed",
            "notification_delivery_failed",
        ])
    );
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
    assert_eq!(OperationsAlertSource::parse("host"), None);
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

/// 与 Worker 默认值一致的策略；测试只改变需要验证的阈值之外的部分。
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

/// 所有新增来源的空水位：没有异常时不得产生任何 signal。
fn quiet_snapshot() -> OperationsAlertSnapshot {
    OperationsAlertSnapshot {
        mail: Some(OperationsAlertMailSnapshot {
            last_poll_age_seconds: 30,
            consecutive_failures: 0,
        }),
        credential: Some(OperationsAlertCredentialSnapshot {
            invalid_mail_cookie: 0,
            invalid_external_api: 0,
        }),
        lifecycle: Some(OperationsAlertLifecycleSnapshot {
            phase_overdue_count: 0,
            oldest_phase_overdue_age_seconds: 0,
            proxy_expiry_overdue_count: 0,
            oldest_proxy_overdue_age_seconds: 0,
        }),
        forum: Some(OperationsAlertForumSnapshot {
            sync_failed_count: 0,
            oldest_sync_failed_age_seconds: 0,
            message_failed_count: 0,
            oldest_message_failed_age_seconds: 0,
        }),
        notification: Some(OperationsAlertNotificationSnapshot {
            failed_count: 0,
            oldest_failed_age_seconds: 0,
            postage_insufficient_count: 0,
            oldest_postage_age_seconds: 0,
        }),
        delivery: Some(OperationsAlertDeliverySnapshot {
            pending: 0,
            failed: 0,
            manual_reconcile: 0,
            returned_mail_manual_reconcile: 0,
            claimed: 0,
            oldest_pending_age_seconds: 0,
            oldest_claimed_age_seconds: 0,
            oldest_manual_reconcile_age_seconds: 0,
            oldest_returned_mail_age_seconds: 0,
            last_update_at: None,
        }),
        ..OperationsAlertSnapshot::default()
    }
}

#[test]
fn missing_sources_are_reported_for_every_closed_source() {
    let observations = evaluate_snapshot(&OperationsAlertSnapshot::default(), &policy())
        .expect("空快照必须可评估");
    assert_eq!(observations.len(), OperationsAlertSource::WORKER.len());
    let reported: BTreeSet<&str> =
        observations.iter().map(|observation| observation.source.as_str()).collect();
    for source in OperationsAlertSource::WORKER {
        assert!(reported.contains(source.as_str()), "{} 缺失来源必须被报告", source.as_str());
        let observation = observations
            .iter()
            .find(|observation| observation.source == source)
            .expect("来源必须存在");
        assert!(!observation.available, "{} 缺失时应不可用", source.as_str());
        assert_eq!(observation.signals.len(), 1);
        assert_eq!(observation.signals[0].kind, OperationsAlertKind::SourceUnavailable);
        assert_eq!(observation.signals[0].scope, source.as_str());
    }
}

#[test]
fn quiet_new_sources_produce_no_signals() {
    let observations = evaluate_snapshot(&quiet_snapshot(), &policy()).expect("空水位必须可评估");
    let quiet_sources = [
        OperationsAlertSource::Mail,
        OperationsAlertSource::Credential,
        OperationsAlertSource::Lifecycle,
        OperationsAlertSource::Forum,
        OperationsAlertSource::Notification,
        OperationsAlertSource::Delivery,
    ];
    for observation in observations {
        if !quiet_sources.contains(&observation.source) {
            continue;
        }
        assert!(
            observation.signals.is_empty(),
            "{} 在空水位下不得产生 signal",
            observation.source.as_str()
        );
    }
}

#[test]
fn new_sources_alert_only_after_their_threshold() {
    let mut snapshot = quiet_snapshot();
    // 轮询停滞：未达阈值不告警，达到固定阈值才告警；失败次数非零立即告警。
    snapshot.mail = Some(OperationsAlertMailSnapshot {
        last_poll_age_seconds: OPERATIONS_ALERT_MAIL_POLL_STALL_AGE.as_secs() as i64 - 1,
        consecutive_failures: 0,
    });
    let mail = |snapshot: &OperationsAlertSnapshot| {
        evaluate_snapshot(snapshot, &policy())
            .expect("可评估")
            .into_iter()
            .find(|observation| observation.source == OperationsAlertSource::Mail)
            .expect("mail 来源必须存在")
            .signals
    };
    assert!(mail(&snapshot).is_empty());
    snapshot.mail = Some(OperationsAlertMailSnapshot {
        last_poll_age_seconds: OPERATIONS_ALERT_MAIL_POLL_STALL_AGE.as_secs() as i64,
        consecutive_failures: 0,
    });
    let signals = mail(&snapshot);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].kind, OperationsAlertKind::MailPollStalled);
    assert_eq!(signals[0].scope, "mail");

    snapshot.mail =
        Some(OperationsAlertMailSnapshot { last_poll_age_seconds: 5, consecutive_failures: 2 });
    let signals = mail(&snapshot);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].count, 2);

    // 生命周期逾期：宽限期内不告警。
    let lifecycle = |age: i64| {
        let mut candidate = quiet_snapshot();
        candidate.lifecycle = Some(OperationsAlertLifecycleSnapshot {
            phase_overdue_count: 1,
            oldest_phase_overdue_age_seconds: age,
            proxy_expiry_overdue_count: 1,
            oldest_proxy_overdue_age_seconds: age,
        });
        evaluate_snapshot(&candidate, &policy())
            .expect("可评估")
            .into_iter()
            .find(|observation| observation.source == OperationsAlertSource::Lifecycle)
            .expect("lifecycle 来源必须存在")
            .signals
    };
    assert!(lifecycle(OPERATIONS_ALERT_LIFECYCLE_GRACE.as_secs() as i64 - 1).is_empty());
    let signals = lifecycle(OPERATIONS_ALERT_LIFECYCLE_GRACE.as_secs() as i64);
    assert_eq!(
        signals.iter().map(|signal| signal.kind).collect::<Vec<_>>(),
        vec![OperationsAlertKind::AuctionPhaseOverdue, OperationsAlertKind::ProxyExpiryOverdue]
    );

    // 队列延迟按领取年龄判断，未达阈值时即使有已领取投递也不告警。
    let delivery = |age: i64| {
        let mut candidate = quiet_snapshot();
        candidate.delivery = Some(OperationsAlertDeliverySnapshot {
            pending: 0,
            failed: 0,
            manual_reconcile: 0,
            returned_mail_manual_reconcile: 0,
            claimed: 3,
            oldest_pending_age_seconds: 0,
            oldest_claimed_age_seconds: age,
            oldest_manual_reconcile_age_seconds: 0,
            oldest_returned_mail_age_seconds: 0,
            last_update_at: None,
        });
        evaluate_snapshot(&candidate, &policy())
            .expect("可评估")
            .into_iter()
            .find(|observation| observation.source == OperationsAlertSource::Delivery)
            .expect("delivery 来源必须存在")
            .signals
    };
    assert!(delivery(OPERATIONS_ALERT_QUEUE_LATENCY_AGE.as_secs() as i64 - 1).is_empty());
    let signals = delivery(OPERATIONS_ALERT_QUEUE_LATENCY_AGE.as_secs() as i64);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].kind, OperationsAlertKind::QueueLatency);
    assert_eq!(signals[0].count, 3);
}

#[test]
fn credential_forum_and_notification_signals_keep_their_scope() {
    let mut snapshot = quiet_snapshot();
    snapshot.credential =
        Some(OperationsAlertCredentialSnapshot { invalid_mail_cookie: 1, invalid_external_api: 2 });
    snapshot.forum = Some(OperationsAlertForumSnapshot {
        sync_failed_count: 1,
        oldest_sync_failed_age_seconds: 600,
        message_failed_count: 2,
        oldest_message_failed_age_seconds: 300,
    });
    snapshot.notification = Some(OperationsAlertNotificationSnapshot {
        failed_count: 4,
        oldest_failed_age_seconds: 900,
        postage_insufficient_count: 1,
        oldest_postage_age_seconds: 700,
    });
    let observations = evaluate_snapshot(&snapshot, &policy()).expect("可评估");
    let signals = |source: OperationsAlertSource| {
        observations
            .iter()
            .find(|observation| observation.source == source)
            .expect("来源必须存在")
            .signals
            .clone()
    };
    let credential = signals(OperationsAlertSource::Credential);
    assert_eq!(credential.len(), 2, "每种凭据各自一条 signal");
    let scopes: BTreeSet<&str> = credential.iter().map(|signal| signal.scope.as_str()).collect();
    assert_eq!(scopes, BTreeSet::from(["external_api", "mail_cookie"]));

    let forum = signals(OperationsAlertSource::Forum);
    assert_eq!(
        forum.iter().map(|signal| signal.kind).collect::<Vec<_>>(),
        vec![OperationsAlertKind::ForumSyncFailed, OperationsAlertKind::ForumMessageFailed]
    );
    // 同一来源内 fingerprint 唯一：去重不需要依赖 upsert 顺序。
    assert_ne!(forum[0].fingerprint, forum[1].fingerprint);

    let notification = signals(OperationsAlertSource::Notification);
    assert_eq!(
        notification.iter().map(|signal| signal.kind).collect::<Vec<_>>(),
        vec![
            OperationsAlertKind::NotificationPostageInsufficient,
            OperationsAlertKind::NotificationDeliveryFailed,
        ]
    );
    assert_eq!(notification[1].count, 4, "邮资不足仍计入总失败水位");
    for signal in &notification {
        assert_eq!(signal.scope, "notification");
        assert!(signal.render_summary().is_ok());
    }
}

#[test]
fn new_kind_summaries_are_single_line_and_bounded() {
    for kind in OperationsAlertKind::ALL {
        let (source, scope) = canonical_identity(kind);
        for (count, age) in [(0_i64, 0_i64), (3, 90_000)] {
            let summary = OperationsAlertSignal::new(kind, scope, source, count, age)
                .expect("规范身份必须有效")
                .render_summary()
                .expect("摘要必须可生成");
            assert!(!summary.is_empty());
            assert!(!summary.contains('\n') && !summary.contains('\r'));
            assert!(summary.len() <= 512);
        }
    }
}

#[test]
fn observe_command_requires_the_full_source_closed_set_in_order() {
    use auction_application::operations::OperationsAlertObserveCommand;
    use chrono::Utc;

    let observations = evaluate_snapshot(&OperationsAlertSnapshot::default(), &policy())
        .expect("空快照必须可评估");
    assert_eq!(observations.len(), OperationsAlertSource::WORKER.len());
    let command = OperationsAlertObserveCommand {
        observations: observations.clone(),
        now: Utc::now(),
        batch_window: Duration::from_secs(120),
    };
    assert_eq!(command.validate(), Ok(()), "每轮提交完整来源闭集必须被接受");

    // 旧的四来源名单会拒绝新来源；这里锁定“缺一个来源即失败关闭”。
    let truncated = OperationsAlertObserveCommand {
        observations: observations[..4].to_vec(),
        now: Utc::now(),
        batch_window: Duration::from_secs(120),
    };
    assert_eq!(truncated.validate(), Err(OperationsAlertError::Invalid));

    // 顺序错误同样拒绝：来源顺序由闭集定义，不能随机排列。
    let mut reordered = observations;
    reordered.swap(0, 1);
    let command = OperationsAlertObserveCommand {
        observations: reordered,
        now: Utc::now(),
        batch_window: Duration::from_secs(120),
    };
    assert_eq!(command.validate(), Err(OperationsAlertError::Invalid));
}

/// 备份来源的磁盘门禁与定时备份终态：字节与比例下限同时满足才算正常。
fn backup_facts(
    available_bytes: u64,
    outcome: OperationsAlertHostBackupOutcome,
) -> OperationsAlertHostBackupFacts {
    OperationsAlertHostBackupFacts {
        disk: Some(OperationsAlertHostDisk {
            total_bytes: 1_000_000,
            available_bytes,
            min_free_bytes: 100_000,
            min_free_percent: 20,
        }),
        latest_run: Some(OperationsAlertHostBackupRun { outcome, age_seconds: 3_600 }),
    }
}

#[test]
fn host_backup_reports_disk_shortfall_and_failed_receipt() {
    // 比例下限 20%（200 000 字节）高于字节下限 100 000，因此缺口按比例下限计算。
    let observation =
        evaluate_host_backup(&backup_facts(150_000, OperationsAlertHostBackupOutcome::Failed))
            .expect("备份事实必须可评估");
    assert!(observation.available);
    assert_eq!(observation.source, OperationsAlertSource::Backup);
    let kinds: Vec<OperationsAlertKind> = observation.signals.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![OperationsAlertKind::DiskCapacityLow, OperationsAlertKind::BackupFailed]
    );
    assert_eq!(observation.signals[0].scope, "backup");
    // 缺口 = 200 000 - 150 000。
    assert_eq!(observation.signals[0].count, 50_000);
    assert_eq!(observation.signals[1].age_seconds, 3_600);
    assert_eq!(
        observation.signals[1].severity(),
        OperationsAlertSeverity::Urgent,
        "备份失败与磁盘水位不会自愈，保持紧急级"
    );
}

#[test]
fn host_backup_completed_and_above_threshold_is_quiet() {
    let observation =
        evaluate_host_backup(&backup_facts(500_000, OperationsAlertHostBackupOutcome::Completed))
            .expect("备份事实必须可评估");
    assert!(observation.available);
    assert!(observation.signals.is_empty());
}

#[test]
fn host_backup_interrupted_receipt_alerts_without_disk_signal() {
    let observation =
        evaluate_host_backup(&backup_facts(250_000, OperationsAlertHostBackupOutcome::Interrupted))
            .expect("备份事实必须可评估");
    let kinds: Vec<OperationsAlertKind> = observation.signals.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, vec![OperationsAlertKind::BackupFailed]);
}

#[test]
fn host_backup_missing_evidence_stays_unavailable_and_keeps_known_signals() {
    let missing_receipt = OperationsAlertHostBackupFacts {
        disk: Some(OperationsAlertHostDisk {
            total_bytes: 1_000_000,
            available_bytes: 10_000,
            min_free_bytes: 100_000,
            min_free_percent: 20,
        }),
        latest_run: None,
    };
    let observation = evaluate_host_backup(&missing_receipt).expect("备份事实必须可评估");
    assert!(!observation.available, "缺少终态回执时来源必须标记为不完整");
    assert_eq!(
        observation.signals[0].kind,
        OperationsAlertKind::SourceUnavailable,
        "不完整来源必须以固定 source_unavailable 开头"
    );
    assert!(
        observation
            .signals
            .iter()
            .any(|signal| signal.kind == OperationsAlertKind::DiskCapacityLow),
        "已确认的磁盘异常不能被证据缺失掩盖"
    );

    let missing_disk = OperationsAlertHostBackupFacts {
        disk: None,
        latest_run: Some(OperationsAlertHostBackupRun {
            outcome: OperationsAlertHostBackupOutcome::Completed,
            age_seconds: 60,
        }),
    };
    let observation = evaluate_host_backup(&missing_disk).expect("备份事实必须可评估");
    assert!(!observation.available);
    assert_eq!(observation.signals.len(), 1);
}

#[test]
fn host_backup_rejects_invalid_thresholds_and_negative_age() {
    let mut facts = backup_facts(500_000, OperationsAlertHostBackupOutcome::Completed);
    facts.disk = Some(OperationsAlertHostDisk {
        total_bytes: 1_000_000,
        available_bytes: 500_000,
        min_free_bytes: 100_000,
        min_free_percent: 0,
    });
    assert_eq!(evaluate_host_backup(&facts), Err(OperationsAlertError::Invalid));
    let mut facts = backup_facts(500_000, OperationsAlertHostBackupOutcome::Failed);
    facts.latest_run = Some(OperationsAlertHostBackupRun {
        outcome: OperationsAlertHostBackupOutcome::Failed,
        age_seconds: -1,
    });
    assert_eq!(evaluate_host_backup(&facts), Err(OperationsAlertError::Invalid));
}

#[test]
fn host_wal_alerts_only_after_the_fixed_stall_window() {
    let quiet = evaluate_host_wal(&OperationsAlertHostWalFacts {
        staged_segments: 3,
        oldest_staged_age_seconds: OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE.as_secs() as i64 - 1,
    })
    .expect("WAL 事实必须可评估");
    assert!(quiet.available);
    assert!(quiet.signals.is_empty());

    let stalled = evaluate_host_wal(&OperationsAlertHostWalFacts {
        staged_segments: 3,
        oldest_staged_age_seconds: OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE.as_secs() as i64,
    })
    .expect("WAL 事实必须可评估");
    assert!(stalled.available);
    assert_eq!(stalled.signals.len(), 1);
    assert_eq!(stalled.signals[0].kind, OperationsAlertKind::WalUploadStalled);
    assert_eq!(stalled.signals[0].scope, "wal");
    assert_eq!(stalled.signals[0].count, 3);
    assert_eq!(
        stalled.signals[0].age_seconds,
        OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE.as_secs() as i64
    );
}

#[test]
fn host_wal_rejects_inconsistent_or_negative_facts() {
    assert_eq!(
        evaluate_host_wal(&OperationsAlertHostWalFacts {
            staged_segments: 0,
            oldest_staged_age_seconds: 10,
        }),
        Err(OperationsAlertError::Invalid),
        "没有暂存段时不存在年龄"
    );
    assert_eq!(
        evaluate_host_wal(&OperationsAlertHostWalFacts {
            staged_segments: -1,
            oldest_staged_age_seconds: 0,
        }),
        Err(OperationsAlertError::Invalid)
    );
}

/// 主机侧命令只接受 host 来源，顺序固定，重复运行命中同一 fingerprint（upsert 去重）。
#[test]
fn host_observe_command_dedupes_cli_retries_by_fingerprint() {
    use chrono::Utc;

    let facts = backup_facts(10_000, OperationsAlertHostBackupOutcome::Failed);
    let first = evaluate_host_backup(&facts).expect("备份事实必须可评估");
    let retry = evaluate_host_backup(&facts).expect("备份事实必须可评估");
    let first_fingerprints: Vec<[u8; 32]> =
        first.signals.iter().map(|signal| signal.fingerprint).collect();
    let retry_fingerprints: Vec<[u8; 32]> =
        retry.signals.iter().map(|signal| signal.fingerprint).collect();
    assert_eq!(
        first_fingerprints, retry_fingerprints,
        "相同事实的重试必须命中同一 signal，而不是新建第二条"
    );

    let wal = evaluate_host_wal(&OperationsAlertHostWalFacts {
        staged_segments: 1,
        oldest_staged_age_seconds: 3_600,
    })
    .expect("WAL 事实必须可评估");
    let command = OperationsAlertHostObserveCommand {
        observations: vec![first, wal],
        now: Utc::now(),
        batch_window: Duration::from_secs(120),
    };
    assert_eq!(command.validate(), Ok(()));

    let reversed = OperationsAlertHostObserveCommand {
        observations: vec![command.observations[1].clone(), command.observations[0].clone()],
        now: Utc::now(),
        batch_window: Duration::from_secs(120),
    };
    assert_eq!(reversed.validate(), Err(OperationsAlertError::Invalid));

    let worker_source = OperationsAlertHostObserveCommand {
        observations: vec![
            OperationsAlertSourceObservation::unavailable(OperationsAlertSource::Mail)
                .expect("不可用观察必须可构造"),
        ],
        now: Utc::now(),
        batch_window: Duration::from_secs(120),
    };
    assert_eq!(
        worker_source.validate(),
        Err(OperationsAlertError::Invalid),
        "主机侧入口不得写入 Worker 来源"
    );

    let empty = OperationsAlertHostObserveCommand {
        observations: Vec::new(),
        now: Utc::now(),
        batch_window: Duration::from_secs(120),
    };
    assert_eq!(empty.validate(), Err(OperationsAlertError::Invalid));
}

#[test]
fn host_sources_are_not_part_of_the_worker_scan_closed_set() {
    for source in OperationsAlertSource::HOST {
        assert!(source.is_host());
        assert!(!OperationsAlertSource::WORKER.contains(&source));
        assert!(OperationsAlertSource::ALL.contains(&source));
        assert!(source.runbook_anchor().starts_with("alert-runbook-"));
    }
    for source in OperationsAlertSource::WORKER {
        assert!(!source.is_host());
    }
}

#[test]
fn reconciliation_reports_drive_drift_shortfall_and_staleness_signals() {
    let policy = policy();
    let snapshot = OperationsAlertSnapshot {
        reconciliation: Some(OperationsAlertReconciliationSnapshot {
            custody_report_age_seconds: Some(600),
            solvency_report_age_seconds: Some(90_000),
            worlds: vec![
                OperationsAlertReconciliationWorld {
                    world: "main",
                    custody_missing_count: 2,
                    solvency_shortfall_credits: 0,
                },
                OperationsAlertReconciliationWorld {
                    world: "isekai",
                    custody_missing_count: 0,
                    solvency_shortfall_credits: 500,
                },
            ],
        }),
        ..quiet_snapshot()
    };
    let observations = evaluate_snapshot(&snapshot, &policy).expect("对账快照必须可评估");
    let reconciliation = observations
        .iter()
        .find(|observation| observation.source == OperationsAlertSource::Reconciliation)
        .expect("必须包含资产对账来源");
    assert!(reconciliation.available);
    let kinds = reconciliation
        .signals
        .iter()
        .map(|signal| (signal.kind, signal.scope.as_str(), signal.count))
        .collect::<Vec<_>>();
    assert!(kinds.contains(&(OperationsAlertKind::CustodyDrift, "main", 2)));
    assert!(kinds.contains(&(OperationsAlertKind::SolvencyShortfall, "isekai", 500)));
    // 24 小时阈值：90_000 秒的偿付能力报告已过期，600 秒的 custody 报告未过期。
    assert!(kinds.contains(&(OperationsAlertKind::ReconciliationStale, "solvency", 1)));
    assert!(!kinds.iter().any(|(kind, scope, _)| {
        *kind == OperationsAlertKind::ReconciliationStale && *scope == "custody"
    }));
}

#[test]
fn missing_reconciliation_reports_are_reported_as_unavailable() {
    let observations = evaluate_snapshot(&quiet_snapshot(), &policy()).expect("空水位必须可评估");
    let reconciliation = observations
        .iter()
        .find(|observation| observation.source == OperationsAlertSource::Reconciliation)
        .expect("必须包含资产对账来源");
    assert!(!reconciliation.available);
    assert_eq!(reconciliation.signals.len(), 1);
    assert_eq!(reconciliation.signals[0].kind, OperationsAlertKind::SourceUnavailable);
}

#[test]
fn awaiting_payment_overdue_without_payment_mail_proof_alerts_without_overdue_stage() {
    // D9 的原始形状：阶段仍是 awaiting_payment，付款期限已过，付款邮件没有发送证明；
    // 此时 payment_overdue_count 为 0，但付款邮件未寄出信号必须独立产生。
    let now = Utc::now();
    let snapshot = OperationsAlertSnapshot {
        recovery: Some(OperationsAlertRecoverySnapshot {
            payment_overdue_count: 0,
            payment_mail_unsent_count: 1,
            relist_pending_count: 0,
            failed_action_count: 0,
            manual_reconcile_count: 0,
            oldest_overdue_age_seconds: 0,
            oldest_payment_mail_unsent_age_seconds: 901,
            worlds: vec![OperationsAlertRecoveryWorld {
                world: "main".to_owned(),
                balance_credits: 1_000_000,
                balance_status: "ok".to_owned(),
                evidence_status: "fresh".to_owned(),
                evidence_age_seconds: 0,
                balance_stall_age_seconds: 0,
                balance_observed_at: Some(now),
                evidence_last_success_at: Some(now),
                updated_at: now,
            }],
        }),
        ..OperationsAlertSnapshot::default()
    };
    let observations = evaluate_snapshot(&snapshot, &policy()).expect("快照必须可评估");
    let recovery = observations
        .iter()
        .find(|observation| observation.source == OperationsAlertSource::Recovery)
        .expect("recovery 来源必须存在");
    assert!(recovery.available);
    let signal = recovery
        .signals
        .iter()
        .find(|signal| signal.kind == OperationsAlertKind::RecoveryPaymentMailUnsent)
        .expect("付款邮件未寄出必须产生独立信号");
    assert_eq!(signal.count, 1);
    assert_eq!(signal.age_seconds, 901);
    assert!(signal.render_summary().is_ok());
}
