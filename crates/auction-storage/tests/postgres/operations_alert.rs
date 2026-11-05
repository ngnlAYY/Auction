//! W5.5 运维告警来源补齐：迁移 000085 的闭集与新增来源水位读取。
//!
//! 只在空库上用真实 PostgreSQL 18 验证两件事：数据库层 kind/scope/source/identity 闭集
//! 对新增取值放行且仍拒绝未登记取值；新增来源的读取查询在已迁移 schema 上真实执行并返回
//! “安静”水位（不产生任何 signal）。不发送通知，也不写入业务事实。
//! 入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[allow(dead_code)]
use crate::support;

use std::time::Duration;

use auction_application::{
    OperationsAlertKind, OperationsAlertPolicy, OperationsAlertSource, OperationsAlertSourceReader,
    evaluate_snapshot,
};
use auction_storage::postgres_expected_migration_version;
use chrono::Utc;

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

/// 插入一条 active signal；调用方给出合法或非法的闭集取值。
async fn insert_signal(
    store: &auction_storage::DatabaseStore,
    seed: &str,
    kind: &str,
    scope: &str,
    source: &str,
) -> Result<(), sqlx::Error> {
    let mut fingerprint = [0_u8; 32];
    let bytes = seed.as_bytes();
    let length = bytes.len().min(32);
    fingerprint[..length].copy_from_slice(&bytes[..length]);
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, count_value, age_seconds, state_version)
         VALUES ($1, $2, $3, $4, true, now(), now(), now() + interval '2 minutes', 1, 0, 1)",
    )
    .bind(fingerprint.as_slice())
    .bind(kind)
    .bind(scope)
    .bind(source)
    .execute(store.postgres_pool())
    .await
    .map(|_| ())
}

#[tokio::test]
async fn postgres_operations_alert_sources_migration_registers_breaking_extension() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");

    // 085 之后的迁移各自追加登记；这里只要求内嵌最高版本已覆盖 085，避免每加一个迁移
    // 都要修改本断言，同时保留“085 必须登记 breaking”的原意。
    assert!(
        postgres_expected_migration_version() >= 85,
        "内嵌迁移必须已包含 W5.5 告警来源扩展（085）"
    );
    let level: String = sqlx::query_scalar(
        "SELECT level FROM public.schema_migration_compatibility WHERE version = 85",
    )
    .fetch_one(store.postgres_pool())
    .await
    .expect("迁移 085 必须登记兼容等级");
    assert_eq!(level, "breaking", "放宽告警闭集后旧程序无法解析新取值");

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_operations_alert_closed_sets_accept_new_kinds_and_reject_unknown() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");

    // 应用层闭集必须与数据库闭集逐项一致：任一新增种类都能以规范身份写入。
    for (index, kind) in OperationsAlertKind::ALL.into_iter().enumerate() {
        use OperationsAlertKind as Kind;
        let (scope, source) = match kind {
            Kind::ServiceMissing | Kind::ServiceDegraded => ("worker", "process"),
            Kind::AuditFailure => ("audit", "process"),
            Kind::SourceUnavailable => ("process", "process"),
            Kind::DeliveryPending
            | Kind::DeliveryReturnedMail
            | Kind::DeliveryManualReconcile
            | Kind::QueueLatency => ("delivery", "delivery"),
            Kind::FulfillmentBacklog
            | Kind::FulfillmentFailed
            | Kind::FulfillmentManualReconcile => ("auction", "fulfillment"),
            Kind::RecoveryPaymentOverdue
            | Kind::RecoveryRelistPending
            | Kind::RecoveryFailed
            | Kind::RecoveryManualReconcile => ("auction", "recovery"),
            Kind::BalanceLow | Kind::EvidenceStale | Kind::AccountBusy => ("main", "recovery"),
            Kind::MailPollStalled => ("mail", "mail"),
            Kind::CredentialInvalid => ("mail_cookie", "credential"),
            Kind::AuctionPhaseOverdue | Kind::ProxyExpiryOverdue => ("auction", "lifecycle"),
            Kind::ForumSyncFailed | Kind::ForumMessageFailed => ("forum", "forum"),
            Kind::NotificationDeliveryFailed | Kind::NotificationPostageInsufficient => {
                ("notification", "notification")
            }
        };
        let seed = format!("kind-{index:02}-{}", kind.as_str());
        insert_signal(&store, &seed, kind.as_str(), scope, source)
            .await
            .unwrap_or_else(|error| panic!("{} 应被数据库闭集接受：{error}", kind.as_str()));
    }

    // 未登记种类与来源在数据库层失败关闭。
    let unknown_kind =
        insert_signal(&store, "unknown-kind", "disk_pressure", "auction", "lifecycle").await;
    assert!(unknown_kind.is_err(), "未登记 kind 必须被拒绝");

    let unknown_source =
        insert_signal(&store, "unknown-source", "mail_poll_stalled", "mail", "backup").await;
    assert!(unknown_source.is_err(), "未登记 source 必须被拒绝");

    // identity 闭集仍生效：来源与 scope 不匹配的新种类同样拒绝。
    let wrong_identity =
        insert_signal(&store, "wrong-identity", "mail_poll_stalled", "auction", "mail").await;
    assert!(wrong_identity.is_err(), "identity 闭集必须拒绝错配来源/范围");

    let wrong_scope =
        insert_signal(&store, "wrong-scope", "credential_invalid", "credential", "credential")
            .await;
    assert!(wrong_scope.is_err(), "credential_invalid 只接受两种凭据种类 scope");

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_operations_alert_new_sources_read_quiet_watermarks() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");

    // 空库上新增来源的读取查询必须真实执行，并返回零水位而不是来源不可用。
    let snapshot = store.snapshot(Utc::now()).await.expect("读取告警来源快照");
    let mail = snapshot.mail.expect("收件轮询水位必须可读");
    assert_eq!(mail.last_poll_age_seconds, 0);
    assert_eq!(mail.consecutive_failures, 0);
    let credential = snapshot.credential.expect("凭据水位必须可读");
    assert_eq!(credential.invalid_mail_cookie, 0);
    assert_eq!(credential.invalid_external_api, 0);
    let lifecycle = snapshot.lifecycle.expect("生命周期水位必须可读");
    assert_eq!(lifecycle.phase_overdue_count, 0);
    assert_eq!(lifecycle.proxy_expiry_overdue_count, 0);
    let forum = snapshot.forum.expect("论坛失败水位必须可读");
    assert_eq!(forum.sync_failed_count, 0);
    assert_eq!(forum.message_failed_count, 0);
    let notification = snapshot.notification.expect("通知失败水位必须可读");
    assert_eq!(notification.failed_count, 0);
    assert_eq!(notification.postage_insufficient_count, 0);
    let delivery = snapshot.delivery.expect("投递水位必须可读");
    assert_eq!(delivery.claimed, 0);
    assert_eq!(delivery.oldest_claimed_age_seconds, 0);

    let observations = evaluate_snapshot(&snapshot, &policy()).expect("快照必须可评估");
    let new_sources = [
        OperationsAlertSource::Mail,
        OperationsAlertSource::Credential,
        OperationsAlertSource::Lifecycle,
        OperationsAlertSource::Forum,
        OperationsAlertSource::Notification,
    ];
    for source in new_sources {
        let observation = observations
            .iter()
            .find(|observation| observation.source == source)
            .unwrap_or_else(|| panic!("{} 来源必须存在", source.as_str()));
        assert!(observation.available, "{} 在空库上应可用", source.as_str());
        assert!(observation.signals.is_empty(), "{} 在空库上不得产生 signal", source.as_str());
    }

    store.postgres_pool().close().await;
}
