//! W5.5 运维告警来源补齐：迁移 000085/000091 的闭集与新增来源水位读取。
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

use auction_application::admin::AdminMailReader;
use auction_application::forum::ForumReplyStore;
use auction_application::operations::{
    OperationsAlertClaimRequest, OperationsAlertConsole, OperationsAlertKind,
    OperationsAlertPolicy, OperationsAlertSource, OperationsAlertSourceReader,
    OperationsAlertStore, OperationsAlertTarget, evaluate_snapshot,
};
use auction_storage::postgres_expected_migration_version;
use chrono::Utc;
use uuid::Uuid;

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

    // 085/091 之后的迁移各自追加登记；这里只要求内嵌最高版本已覆盖 091，避免每加一个
    // 迁移都要修改本断言，同时保留“放宽闭集的迁移必须登记 breaking”的原意。
    assert!(
        postgres_expected_migration_version() >= 101,
        "内嵌迁移必须已包含 W5.5 页数与换季告警扩展（101）"
    );
    for version in [85, 91, 99, 101] {
        let level: String = sqlx::query_scalar(
            "SELECT level FROM public.schema_migration_compatibility WHERE version = $1",
        )
        .bind(version)
        .fetch_one(store.postgres_pool())
        .await
        .unwrap_or_else(|error| panic!("迁移 {version} 必须登记兼容等级：{error}"));
        assert_eq!(level, "breaking", "放宽告警闭集后旧程序无法解析新取值");
    }

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
            | Kind::RecoveryPaymentMailUnsent
            | Kind::RecoveryRelistPending
            | Kind::RecoveryFailed
            | Kind::RecoveryManualReconcile => ("auction", "recovery"),
            Kind::BalanceLow | Kind::EvidenceStale | Kind::AccountBusy => ("main", "recovery"),
            Kind::MailPollStalled | Kind::MailSentPageLimitApproaching => ("mail", "mail"),
            Kind::CredentialInvalid => ("mail_cookie", "credential"),
            Kind::AuctionPhaseOverdue | Kind::ProxyExpiryOverdue => ("auction", "lifecycle"),
            Kind::SeasonObligationsPending => ("isekai", "lifecycle"),
            Kind::ForumSyncFailed
            | Kind::ForumMessageFailed
            | Kind::ForumTopicPageLimitApproaching => ("forum", "forum"),
            Kind::NotificationDeliveryFailed | Kind::NotificationPostageInsufficient => {
                ("notification", "notification")
            }
            Kind::DiskCapacityLow | Kind::BackupFailed => ("backup", "backup"),
            Kind::WalUploadStalled => ("wal", "wal"),
            Kind::CustodyDrift | Kind::SolvencyShortfall => ("main", "reconciliation"),
            Kind::ReconciliationStale => ("custody", "reconciliation"),
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
        insert_signal(&store, "unknown-source", "mail_poll_stalled", "mail", "host").await;
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
    assert_eq!(mail.sent_pages_scanned, None);
    let credential = snapshot.credential.expect("凭据水位必须可读");
    assert_eq!(credential.invalid_mail_cookie, 0);
    assert_eq!(credential.invalid_external_api, 0);
    let lifecycle = snapshot.lifecycle.expect("生命周期水位必须可读");
    assert_eq!(lifecycle.phase_overdue_count, 0);
    assert_eq!(lifecycle.proxy_expiry_overdue_count, 0);
    assert_eq!(lifecycle.season_obligations_pending, 0);
    let forum = snapshot.forum.expect("论坛失败水位必须可读");
    assert_eq!(forum.sync_failed_count, 0);
    assert_eq!(forum.message_failed_count, 0);
    assert_eq!(forum.remote_page_count, None);
    let notification = snapshot.notification.expect("通知失败水位必须可读");
    assert_eq!(notification.failed_count, 0);
    assert_eq!(notification.postage_insufficient_count, 0);
    let recovery = snapshot.recovery.as_ref().expect("恢复水位必须可读");
    assert_eq!(recovery.payment_mail_unsent_count, 0);
    assert_eq!(recovery.oldest_payment_mail_unsent_age_seconds, 0);
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

#[tokio::test]
async fn postgres_payment_mail_unsent_counts_awaiting_payment_after_deadline() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let winner = Uuid::new_v4();
    let auction_id = Uuid::new_v4();
    let lot_id = Uuid::new_v4();
    let equipment_id = Uuid::new_v4();
    let payment_obligation = Uuid::new_v4();
    let seller_obligation = Uuid::new_v4();
    let refund_obligation = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let recovery_id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO public.users (id, username, public_key_hash)
         VALUES ($1, $2, $3)",
    )
    .bind(winner)
    .bind(format!("d9-winner-{winner}"))
    .bind([7_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入测试用户");
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'main', $2, 'D9 test equipment', 'weapon', '{}'::jsonb, $3)",
    )
    .bind(equipment_id)
    .bind(format!("d9-eid-{equipment_id}"))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入测试装备");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', $2, 'D9 test auction', 'settled', $3, $3, $3, $3, $3, $3, $4)",
    )
    .bind(auction_id)
    .bind(i32::try_from(now.timestamp() % 1_000_000).expect("sequence"))
    .bind(now)
    .bind(serde_json::json!({
        "schema_version": 5,
        "start_price": 100,
        "bid_increment_ppm": 100_000,
        "minimum_increment": 100,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 3,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": now,
        "settlement": {
            "version": 2,
            "mode": "hold_curve_v2",
            "compensation_pool_enabled": true,
            "anchors": []
        },
        "recovery": {
            "version": 2,
            "payment_timeout_seconds": 86400,
            "mode": "remind_v1"
        },
        "bidding_version": 2
    }))
    .execute(pool)
    .await
    .expect("写入测试场次");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'sold', 100, 100, $4, 'weapon', 1)",
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("写入测试 lot");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'd9-test', $2, $3, 'accepted')",
    )
    .bind(command_id)
    .bind(auction_id)
    .bind([9_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入测试命令");
    for (id, kind) in [
        (payment_obligation, "winner_equipment_cod"),
        (seller_obligation, "seller_payout"),
        (refund_obligation, "buyer_refund"),
    ] {
        let (business_state, recipient_user, recipient_name, amount, prerequisite) = match kind {
            "winner_equipment_cod" => ("ready", Some(winner), None, 100, None),
            "seller_payout" => ("ready", None, Some("D9 seller"), 90, Some(payment_obligation)),
            _ => ("contingent", Some(winner), None, 100, Some(payment_obligation)),
        };
        sqlx::query(
            "INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_user_id, recipient_game_identity,
                 amount, carrier_credit, equipment_id, prerequisite_obligation_id)
             VALUES ($1, $2, $3, 'main', $4, 1, $5, $6, $7, $8, $9, 0, $10, $11)",
        )
        .bind(id)
        .bind(auction_id)
        .bind(lot_id)
        .bind(command_id)
        .bind(kind)
        .bind(business_state)
        .bind(recipient_user)
        .bind(recipient_name)
        .bind(amount)
        .bind(equipment_id)
        .bind(prerequisite)
        .execute(pool)
        .await
        .expect("写入测试义务");
    }
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
             policy_version, payment_deadline_at, stage, state_version, created_at, updated_at)
         VALUES ($1, $2, $3, 'main', $4, $5, $6, $7, $8, 1, $9, 'awaiting_payment', 1, $10, $11)",
    )
    .bind(recovery_id)
    .bind(auction_id)
    .bind(lot_id)
    .bind(equipment_id)
    .bind(winner)
    .bind(payment_obligation)
    .bind(seller_obligation)
    .bind(refund_obligation)
    .bind(now - chrono::Duration::minutes(5))
    .bind(now - chrono::Duration::hours(1))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入测试恢复");

    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    let recovery = snapshot.recovery.expect("恢复水位");
    assert_eq!(recovery.payment_overdue_count, 0);
    assert_eq!(recovery.payment_mail_unsent_count, 1);
    assert!(recovery.oldest_payment_mail_unsent_age_seconds >= 300);

    store.postgres_pool().close().await;
}

/// 换季告警统计的是旧场未完成义务，不是未来边界场自身的义务。
#[tokio::test]
async fn postgres_season_alert_counts_old_outstanding_obligations_before_future_boundary() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let now = Utc::now();
    let old_auction = Uuid::new_v4();
    let live_auction = Uuid::new_v4();
    let future_auction = Uuid::new_v4();
    let equipment_id = Uuid::new_v4();
    let lot_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let obligation_id = Uuid::new_v4();
    let policy = serde_json::json!({
        "schema_version": 5,
        "start_price": 100,
        "bid_increment_ppm": 100_000,
        "minimum_increment": 100,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 3,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": now,
        "settlement": {
            "version": 2,
            "mode": "hold_curve_v2",
            "compensation_pool_enabled": true,
            "anchors": []
        },
        "recovery": {"version": 2, "payment_timeout_seconds": 86400, "mode": "remind_v1"},
        "bidding_version": 2
    });

    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'isekai', $2, 'season test equipment', 'weapon', '{}'::jsonb, $3)",
    )
    .bind(equipment_id)
    .bind(format!("season-eid-{equipment_id}"))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入换季测试装备");
    for (id, status, intake) in [(old_auction, "settled", None::<chrono::DateTime<Utc>>)] {
        sqlx::query(
            "INSERT INTO public.auctions
                (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
                 scheduled_end_at, effective_end_at, preview_opened_at, opened_at,
                 intake_opens_at, policy_snapshot)
             VALUES ($1, 'isekai', $2, $3, $4, $5, $5, $5, $5, $5, $5, $6, $7)",
        )
        .bind(id)
        .bind(
            i32::try_from(now.timestamp() % 1_000_000).expect("sequence")
                + i32::from(id == future_auction),
        )
        .bind(format!("season test {id}"))
        .bind(status)
        .bind(now)
        .bind(intake)
        .bind(&policy)
        .execute(pool)
        .await
        .expect("写入换季测试场次");
    }
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at,
             intake_opens_at, policy_snapshot)
         VALUES ($1, 'isekai', $2, 'season live forum', 'live', $3, $3, $3, $3, $3, $3,
                 NULL, $4)",
    )
    .bind(live_auction)
    .bind(i32::try_from(now.timestamp() % 1_000_000).expect("sequence") + 2)
    .bind(now)
    .bind(&policy)
    .execute(pool)
    .await
    .expect("写入论坛可扫描场次");
    sqlx::query(
        "INSERT INTO public.auction_forum_sync
            (auction_id, status, thread_id, thread_url, title, body,
             remote_page_count, remote_page_limit, remote_page_count_observed_at,
             terminal_refresh_enabled)
         VALUES ($1, 'published', '1', 'https://example.invalid/thread/1', 't', 'b', 8, 10, $2, false),
                ($3, 'published', '2', 'https://example.invalid/thread/2', 't', 'b', 9, 10, $2, true)",
    )
    .bind(old_auction)
    .bind(now)
    .bind(live_auction)
    .execute(pool)
    .await
    .expect("写入论坛页数观察");

    // 未来 intake 场次与 target 必须在同一事务提交，延迟约束才能看到一致的一对。
    let mut target_tx = pool.begin().await.expect("开启未来边界事务");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at,
             intake_opens_at, policy_snapshot)
         VALUES ($1, 'isekai', $2, $3, 'draft', $4, $4, $4, $4, $4, $4, $5, $6)",
    )
    .bind(future_auction)
    .bind(i32::try_from(now.timestamp() % 1_000_000).expect("sequence") + 1)
    .bind("season future boundary")
    .bind(now)
    .bind(now + chrono::Duration::days(1))
    .bind(&policy)
    .execute(&mut *target_tx)
    .await
    .expect("写入未来换季边界场次");
    sqlx::query(
        "INSERT INTO public.auction_intake_targets (world_id, auction_id, target_status)
         VALUES ('isekai', $1, 'draft')",
    )
    .bind(future_auction)
    .execute(&mut *target_tx)
    .await
    .expect("写入未来换季边界目标");
    target_tx.commit().await.expect("提交未来边界事务");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'sold', 100, 100, $4, 'weapon', 1)",
    )
    .bind(lot_id)
    .bind(old_auction)
    .bind(equipment_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("写入换季测试 lot");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'season-test', $2, $3, 'accepted')",
    )
    .bind(command_id)
    .bind(old_auction)
    .bind([8_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入换季测试命令");
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_game_identity, amount,
             carrier_credit, equipment_id)
         VALUES ($1, $2, $3, 'isekai', $4, 1, 'unsold_return', 'ready',
                 'season seller', 0, 0, $5)",
    )
    .bind(obligation_id)
    .bind(old_auction)
    .bind(lot_id)
    .bind(command_id)
    .bind(equipment_id)
    .execute(pool)
    .await
    .expect("写入换季测试义务");

    let snapshot = store.snapshot(now).await.expect("读取换季告警快照");
    let forum = snapshot.forum.expect("论坛快照");
    assert_eq!(forum.remote_page_count, Some(9), "论坛页数只能读取可扫描场次");
    assert_eq!(forum.remote_page_limit, Some(10), "论坛页数必须携带有效上限");
    let updated_before: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT updated_at FROM public.auction_forum_sync WHERE auction_id = $1",
    )
    .bind(live_auction)
    .fetch_one(pool)
    .await
    .expect("读取论坛业务更新时间");
    store
        .record_topic_page_count(auction_domain::AuctionId::from_uuid(live_auction), 10, 10)
        .await
        .expect("推进论坛页数观察");
    let updated_after: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT updated_at FROM public.auction_forum_sync WHERE auction_id = $1",
    )
    .bind(live_auction)
    .fetch_one(pool)
    .await
    .expect("读取论坛业务更新时间");
    assert_eq!(updated_before, updated_after, "页数观察不能重置论坛业务失败年龄");
    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    let forum = snapshot.forum.expect("论坛快照");
    assert_eq!(forum.remote_page_count, Some(10));
    assert_eq!(forum.remote_page_limit, Some(10));

    // 超限失败仍可保存真实页数，不能改写成上限或伪造 max+1。
    store
        .record_topic_page_count(auction_domain::AuctionId::from_uuid(live_auction), 12, 10)
        .await
        .expect("记录超限论坛页数");
    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    let forum = snapshot.forum.expect("论坛快照");
    assert_eq!(forum.remote_page_count, Some(12));
    assert_eq!(forum.remote_page_limit, Some(10));

    // 超过 7 天的页数观察不能被当作当前远端页数。
    sqlx::query(
        "UPDATE public.auction_forum_sync
         SET remote_page_count_observed_at = $2
         WHERE auction_id = $1",
    )
    .bind(live_auction)
    .bind(now - chrono::Duration::days(8))
    .execute(pool)
    .await
    .expect("设置过期论坛页数观察");
    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    assert_eq!(
        snapshot.forum.expect("论坛快照").remote_page_count,
        None,
        "过期或旧场页数观察不能产生论坛告警"
    );
    assert_eq!(
        snapshot.lifecycle.expect("生命周期水位").season_obligations_pending,
        1,
        "旧场 pending 义务必须计入未来冻结边界前的换季告警"
    );

    // contingent 退款尚未触发，不能当作当前换季欠付。
    let buyer_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.users (id, username, public_key_hash)
         VALUES ($1, 'season contingent buyer', $2)",
    )
    .bind(buyer_id)
    .bind([9_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入换季 contingent 买家");
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_user_id, amount, carrier_credit,
             equipment_id, prerequisite_obligation_id)
         VALUES ($1, $2, $3, 'isekai', $4, 1, 'buyer_refund', 'contingent', $5,
                 100, 0, $6, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(old_auction)
    .bind(lot_id)
    .bind(command_id)
    .bind(buyer_id)
    .bind(equipment_id)
    .bind(obligation_id)
    .execute(pool)
    .await
    .expect("写入换季 contingent 退款");
    let snapshot = store.snapshot(now).await.expect("读取换季告警快照");
    assert_eq!(
        snapshot.lifecycle.expect("生命周期水位").season_obligations_pending,
        1,
        "未触发的 contingent 退款不能计入换季欠付"
    );

    // manual_reconcile 仍可能欠装备或款项，必须保留在换季告警中。
    let manual_seller_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_game_identity, amount, carrier_credit,
             equipment_id, prerequisite_obligation_id)
         VALUES ($1, $2, $3, 'isekai', $4, 1, 'seller_payout', 'ready', 'season seller',
                 100, 0, $5, $6)",
    )
    .bind(manual_seller_id)
    .bind(old_auction)
    .bind(lot_id)
    .bind(command_id)
    .bind(equipment_id)
    .bind(obligation_id)
    .execute(pool)
    .await
    .expect("写入换季人工对账义务");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation, completed_at)
         VALUES ($1, 'manual_reconcile', 'isekai', 'send', $2)",
    )
    .bind(manual_seller_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("写入换季人工对账 fulfillment");
    let snapshot = store.snapshot(now).await.expect("读取换季告警快照");
    assert_eq!(
        snapshot.lifecycle.expect("生命周期水位").season_obligations_pending,
        2,
        "manual_reconcile 义务仍需人工收口，不能从换季告警消失"
    );

    // failed 仍可能欠付；只有已正式处置的失败才由损失事实排除。
    let failed_return_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_game_identity, amount, carrier_credit,
             equipment_id)
         VALUES ($1, $2, $3, 'isekai', $4, 1, 'withdrawn_return', 'ready', 'season seller',
                 0, 0, $5)",
    )
    .bind(failed_return_id)
    .bind(old_auction)
    .bind(lot_id)
    .bind(command_id)
    .bind(equipment_id)
    .execute(pool)
    .await
    .expect("写入换季 failed 义务");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, dispatch_world, delivery_operation, completed_at)
         VALUES ($1, 'failed', 'isekai', 'return', $2)",
    )
    .bind(failed_return_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("写入换季 failed fulfillment");
    let snapshot = store.snapshot(now).await.expect("读取换季告警快照");
    assert_eq!(
        snapshot.lifecycle.expect("生命周期水位").season_obligations_pending,
        3,
        "仍欠付的 failed 义务必须计入换季告警"
    );
    store.postgres_pool().close().await;
}

/// Sent 页数只读取当前 active 凭据的近期完整枚举；旧凭据与过期观察不能冒充。
#[tokio::test]
async fn postgres_sent_page_count_is_scoped_to_current_credential_and_recent_scan() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let now = Utc::now();
    let old_credential = Uuid::new_v4();
    let current_credential = Uuid::new_v4();
    for (id, label, fingerprint, status, version, created_at) in [
        (
            old_credential,
            "old",
            "fingerprint-old",
            "disabled",
            1_i64,
            now - chrono::Duration::days(2),
        ),
        (current_credential, "current", "fingerprint-current", "active", 2_i64, now),
    ] {
        sqlx::query(
            "INSERT INTO public.credential_keys
                (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id,
                 fingerprint, status, state_version, created_at)
             VALUES ($1, 'mail_cookie', 'main', $2, $3, $4, 'test-key', $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(label)
        .bind(vec![1_u8])
        .bind(vec![0_u8; 12])
        .bind(fingerprint)
        .bind(status)
        .bind(version)
        .bind(created_at)
        .execute(pool)
        .await
        .expect("写入页数测试凭据");
    }
    for (credential, version, pages, observed_at) in [
        (old_credential, 1_i64, 99_i32, now - chrono::Duration::days(1)),
        (current_credential, 2_i64, 16_i32, now),
    ] {
        sqlx::query(
            "INSERT INTO public.mail_scan_checkpoints
                (credential_id, world_id, stream, credential_version, endpoint_scope,
                 last_full_pages_scanned, last_full_page_limit, last_full_pages_observed_at)
             VALUES ($1, 'main', 'sent', $2, 'mail.example', $3, 20, $4)",
        )
        .bind(credential)
        .bind(version)
        .bind(pages)
        .bind(observed_at)
        .execute(pool)
        .await
        .expect("写入 Sent 页数检查点");
    }
    sqlx::query(
        "INSERT INTO public.mail_poll_state
            (mailbox_key, last_poll_at, next_poll_at, consecutive_failures, state_version)
         VALUES ('main', $1, $1, 0, 1)",
    )
    .bind(now - chrono::Duration::seconds(30))
    .execute(pool)
    .await
    .expect("写入邮件轮询状态");

    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    let mail = snapshot.mail.as_ref().expect("邮件快照");
    assert_eq!(mail.sent_pages_scanned, Some(16), "只能读取当前 active 凭据的完整枚举页数");
    assert_eq!(mail.sent_page_limit, Some(20), "Sent 页数必须携带有效上限");

    // 当前凭据的完整枚举超过 7 天后必须视为过期，不回退到旧凭据的页数。
    sqlx::query(
        "UPDATE public.mail_scan_checkpoints
         SET last_full_pages_observed_at = $2
         WHERE credential_id = $1",
    )
    .bind(current_credential)
    .bind(now - chrono::Duration::days(8))
    .execute(pool)
    .await
    .expect("设置过期页数观察");
    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    assert_eq!(
        snapshot.mail.as_ref().expect("邮件快照").sent_pages_scanned,
        None,
        "过期观察和旧凭据不能产生页数告警"
    );

    // 记录接口只更新精确的 current 作用域行，并保留页数时间。
    let observation = auction_application::mail::MailScanPageObservation {
        credential_id: current_credential,
        credential_version: 2,
        world_id: "main".to_owned(),
        endpoint_scope: "mail.example".to_owned(),
        pages_scanned: 17,
        page_limit: 20,
        observed_at: now,
    };
    use auction_application::mail::MoogleMailboxEvidenceStore;
    store.record_sent_pages(&observation).await.expect("记录页数观察");
    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    assert_eq!(snapshot.mail.as_ref().expect("邮件快照").sent_pages_scanned, Some(17));

    let mut stale = observation.clone();
    stale.pages_scanned = 3;
    stale.observed_at = now - chrono::Duration::days(1);
    store.record_sent_pages(&stale).await.expect("记录过期页数观察");
    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    assert_eq!(
        snapshot.mail.as_ref().expect("邮件快照").sent_pages_scanned,
        Some(17),
        "迟到的旧观察不能回退当前页数"
    );

    // 凭据版本或 endpoint 变化时，旧页数观察必须随检查点作用域切换一起失效。
    use auction_application::mail::{
        MailScanCheckpoint, MailScanCheckpointKey, MailScanCheckpointStore, MailScanStream,
    };
    let key = MailScanCheckpointKey {
        credential_id: current_credential,
        credential_version: 3,
        world_id: "main".to_owned(),
        stream: MailScanStream::Sent,
        endpoint_scope: "mail.example".to_owned(),
    };
    store
        .save(&key, &MailScanCheckpoint { watermark: None, last_full_scan_at: Some(now) })
        .await
        .expect("推进检查点作用域");
    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    assert_eq!(
        snapshot.mail.as_ref().expect("邮件快照").sent_pages_scanned,
        None,
        "换版本后旧页数不能沿用"
    );

    pool.close().await;
}

#[tokio::test]
async fn postgres_operations_alert_webhook_ledger_claims_and_finishes_independently() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    // 模拟备份恢复后的 --no-owner 路径：函数必须重新授予新表写权限。
    sqlx::query("SELECT public.apply_auction_runtime_privileges()")
        .execute(store.postgres_pool())
        .await
        .expect("重放运行账号授权函数");
    let writable: bool = sqlx::query_scalar(
        "SELECT has_table_privilege(
             'auction_runtime',
             'public.operations_alert_webhook_deliveries',
             'SELECT,INSERT,UPDATE,DELETE'
         )",
    )
    .fetch_one(store.postgres_pool())
    .await
    .expect("检查 webhook 账本授权");
    assert!(writable, "运行账号必须能领取并终结 webhook 投递");
    let mut fingerprint = [0_u8; 32];
    fingerprint[..7].copy_from_slice(b"webhook");
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, next_webhook_at, count_value, age_seconds, state_version)
         VALUES ($1, 'service_missing', 'worker', 'process', true, now(), now(),
                 now() + interval '30 minutes', now() - interval '1 second', 2, 7, 1)",
    )
    .bind(fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("写入 webhook 来源 signal");

    let now = Utc::now();
    let request = auction_application::operations::OperationsAlertWebhookClaimRequest {
        now,
        lease_until: now + chrono::Duration::seconds(120),
        template: auction_application::operations::DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE
            .to_owned(),
    };
    let dispatch =
        store.claim_due_webhook(request).await.expect("领取 webhook").expect("存在到期 signal");
    assert_eq!(dispatch.kind, OperationsAlertKind::ServiceMissing);
    assert_eq!(dispatch.payload["count"], 2);
    assert_eq!(dispatch.payload["age_seconds"], 7);
    let status: String = sqlx::query_scalar(
        "SELECT status FROM public.operations_alert_webhook_deliveries WHERE id = $1",
    )
    .bind(dispatch.delivery_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取投递状态");
    assert_eq!(status, "sending");

    let next = now + chrono::Duration::minutes(30);
    store
        .finish_webhook(
            dispatch,
            auction_application::operations::OperationsAlertWebhookFinish {
                outcome: auction_application::operations::OperationsAlertWebhookOutcome::Succeeded,
                next_webhook_at: Some(next),
            },
        )
        .await
        .expect("写入成功终态");
    let (status, next_webhook_at): (String, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT d.status, s.next_webhook_at
         FROM public.operations_alert_webhook_deliveries AS d
         JOIN public.operations_alert_signals AS s ON s.fingerprint = d.fingerprint
         WHERE d.fingerprint = $1",
    )
    .bind(fingerprint.as_slice())
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取 webhook 成功终态");
    assert_eq!(status, "succeeded");
    assert_eq!(
        next_webhook_at.expect("冷却时间必须保留").timestamp_micros(),
        next.timestamp_micros(),
        "PostgreSQL 只保留微秒精度"
    );

    // 未知结果进入 manual_reconcile，且 signal 冷却保持 NULL，不能自动重发。
    let mut unknown_fingerprint = [0_u8; 32];
    unknown_fingerprint[..12].copy_from_slice(b"webhook-unkn");
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, next_webhook_at, count_value, age_seconds, state_version)
         VALUES ($1, 'service_degraded', 'worker', 'process', true, now(), now(),
                 now() + interval '30 minutes', now() - interval '1 second', 1, 3, 1)",
    )
    .bind(unknown_fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("写入第二 webhook signal");
    let now = Utc::now();
    let unknown_dispatch = store
        .claim_due_webhook(auction_application::operations::OperationsAlertWebhookClaimRequest {
            now,
            lease_until: now + chrono::Duration::seconds(120),
            template: auction_application::operations::DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE
                .to_owned(),
        })
        .await
        .expect("领取第二 webhook")
        .expect("第二 signal 到期");
    store
        .finish_webhook(
            unknown_dispatch,
            auction_application::operations::OperationsAlertWebhookFinish {
                outcome: auction_application::operations::OperationsAlertWebhookOutcome::Unknown,
                next_webhook_at: None,
            },
        )
        .await
        .expect("写入未知终态");
    let (status, next_webhook_at): (String, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT d.status, s.next_webhook_at
         FROM public.operations_alert_webhook_deliveries AS d
         JOIN public.operations_alert_signals AS s ON s.fingerprint = d.fingerprint
         WHERE d.fingerprint = $1",
    )
    .bind(unknown_fingerprint.as_slice())
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取未知终态");
    assert_eq!(status, "manual_reconcile");
    assert_eq!(next_webhook_at, None);

    // 发送租约过期时，下一次领取必须先把旧行收口为 manual_reconcile，不能自动重发。
    let mut stale_fingerprint = [0_u8; 32];
    stale_fingerprint[..10].copy_from_slice(b"webhook-st");
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, next_webhook_at, count_value, age_seconds, state_version)
         VALUES ($1, 'queue_latency', 'delivery', 'delivery', true, now(), now(),
                 now() + interval '30 minutes', now() - interval '1 second', 1, 2, 1)",
    )
    .bind(stale_fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("写入租约过期 signal");
    let now = Utc::now();
    let stale_dispatch = store
        .claim_due_webhook(auction_application::operations::OperationsAlertWebhookClaimRequest {
            now,
            lease_until: now + chrono::Duration::seconds(120),
            template: auction_application::operations::DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE
                .to_owned(),
        })
        .await
        .expect("领取租约过期 signal")
        .expect("第三 signal 到期");
    sqlx::query(
        "UPDATE public.operations_alert_webhook_deliveries
         SET claim_expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(stale_dispatch.delivery_id)
    .execute(store.postgres_pool())
    .await
    .expect("提前租约");
    let _ = store
        .claim_due_webhook(auction_application::operations::OperationsAlertWebhookClaimRequest {
            now: Utc::now(),
            lease_until: Utc::now() + chrono::Duration::seconds(120),
            template: auction_application::operations::DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE
                .to_owned(),
        })
        .await
        .expect("触发过期收口");
    let stale_status: String = sqlx::query_scalar(
        "SELECT status FROM public.operations_alert_webhook_deliveries WHERE id = $1",
    )
    .bind(stale_dispatch.delivery_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取过期状态");
    assert_eq!(stale_status, "manual_reconcile");

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_operations_alert_webhook_retry_uses_signal_kind_without_payload_field() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let mut fingerprint = [0_u8; 32];
    fingerprint[..9].copy_from_slice(b"webhook-r");
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, next_webhook_at, count_value, age_seconds, state_version)
         VALUES ($1, 'service_missing', 'worker', 'process', true, now(), now(),
                 now() + interval '30 minutes', now() - interval '1 second', 1, 4, 1)",
    )
    .bind(fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("写入重试 signal");
    let now = Utc::now();
    let request = |now: chrono::DateTime<Utc>| {
        auction_application::operations::OperationsAlertWebhookClaimRequest {
            now,
            lease_until: now + chrono::Duration::seconds(120),
            template: "{\"format\":\"auction-operations-alert.v1\",\"source\":\"{source}\"}"
                .to_owned(),
        }
    };
    let dispatch =
        store.claim_due_webhook(request(now)).await.expect("领取重试 signal").expect("到期");
    assert_eq!(dispatch.kind, OperationsAlertKind::ServiceMissing);
    let retry_at = now + chrono::Duration::seconds(2);
    store
        .finish_webhook(
            dispatch,
            auction_application::operations::OperationsAlertWebhookFinish {
                outcome: auction_application::operations::OperationsAlertWebhookOutcome::Retryable,
                next_webhook_at: Some(retry_at),
            },
        )
        .await
        .expect("写入可重试终态");
    let retried = store
        .claim_due_webhook(request(retry_at + chrono::Duration::seconds(1)))
        .await
        .expect("领取重试")
        .expect("pending 行应重新到期");
    assert_eq!(retried.kind, OperationsAlertKind::ServiceMissing);
    assert!(retried.payload.get("kind").is_none());
    store
        .finish_webhook(
            retried,
            auction_application::operations::OperationsAlertWebhookFinish {
                outcome: auction_application::operations::OperationsAlertWebhookOutcome::Succeeded,
                next_webhook_at: Some(retry_at + chrono::Duration::minutes(30)),
            },
        )
        .await
        .expect("写入成功终态");
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_operations_alert_webhook_respects_silence_window() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let mut fingerprint = [0_u8; 32];
    fingerprint[..10].copy_from_slice(b"webhook-si");
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, next_webhook_at, count_value, age_seconds, state_version)
         VALUES ($1, 'service_missing', 'worker', 'process', true, now(), now(),
                 now() + interval '30 minutes', now() - interval '1 second', 1, 5, 1)",
    )
    .bind(fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("写入待静默 signal");

    // 先完成一次成功投递，模拟“已成功提醒后”被静默的 signal。
    let first_now = Utc::now();
    let first = store
        .claim_due_webhook(auction_application::operations::OperationsAlertWebhookClaimRequest {
            now: first_now,
            lease_until: first_now + chrono::Duration::seconds(120),
            template: auction_application::operations::DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE
                .to_owned(),
        })
        .await
        .expect("首次领取")
        .expect("首次投递必须到期");
    store
        .finish_webhook(
            first,
            auction_application::operations::OperationsAlertWebhookFinish {
                outcome: auction_application::operations::OperationsAlertWebhookOutcome::Succeeded,
                next_webhook_at: Some(first_now + chrono::Duration::seconds(1)),
            },
        )
        .await
        .expect("首次投递成功");

    let silence = auction_application::operations::OperationsAlertSilenceCommand::new(
        Uuid::new_v4(),
        fingerprint,
        3_600,
        "operator",
        "planned maintenance",
    )
    .expect("构造静默命令");
    let silenced_until = store
        .silence_alert(silence)
        .await
        .expect("写入静默")
        .silenced_until
        .expect("静默结果必须带截止时间");

    let during_silence = silenced_until - chrono::Duration::seconds(60);
    let after_silence = silenced_until + chrono::Duration::seconds(1);
    assert!(during_silence < silenced_until, "测试时钟必须落在静默窗口内");
    assert!(
        store
            .claim_due_webhook(
                auction_application::operations::OperationsAlertWebhookClaimRequest {
                    now: during_silence,
                    lease_until: during_silence + chrono::Duration::seconds(120),
                    template:
                        auction_application::operations::DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE
                            .to_owned(),
                }
            )
            .await
            .expect("静默窗口内领取")
            .is_none(),
        "静默未到期时不得创建或发送 webhook 重复通知"
    );
    assert!(
        store
            .claim_due_webhook(
                auction_application::operations::OperationsAlertWebhookClaimRequest {
                    now: after_silence,
                    lease_until: after_silence + chrono::Duration::seconds(120),
                    template:
                        auction_application::operations::DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE
                            .to_owned(),
                }
            )
            .await
            .expect("静默到期后领取")
            .is_some(),
        "静默到期后应恢复 webhook 重复通知"
    );
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_operations_alert_claim_keeps_active_summary_and_enqueue_preserves_silence() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    // fingerprint 必须与 canonical 身份一致，否则活动摘要校验会拒绝渲染。
    use sha2::Digest as _;
    let fingerprint: [u8; 32] =
        sha2::Sha256::digest(b"operations-alert:v1\0service_missing\0worker").into();
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, count_value, age_seconds, state_version)
         VALUES ($1, 'service_missing', 'worker', 'process', true,
                 now() - interval '10 seconds', now() - interval '10 seconds',
                 now() - interval '1 second', 1, 5, 1)",
    )
    .bind(fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("写入活动紧急 signal");

    let request = |now: chrono::DateTime<Utc>| OperationsAlertClaimRequest {
        target: OperationsAlertTarget {
            world_id: "main".to_owned(),
            recipient: "operator".to_owned(),
        },
        now,
        lease_until: now + chrono::Duration::seconds(120),
        max_items: 20,
        max_body_bytes: 4_096,
    };
    let now = Utc::now();
    let claim = store
        .claim_due_batch(request(now))
        .await
        .expect("领取活动紧急 signal")
        .expect("到期紧急 signal 必须被领取");
    // 活动故障必须渲染动作摘要；历史缺陷会把紧急批次整批标成“已恢复”。
    assert_eq!(claim.items.len(), 1, "只有一条活动 signal 到期");
    assert!(
        claim.items[0].summary.contains("服务缺失"),
        "活动告警摘要必须描述故障：{}",
        claim.items[0].summary
    );
    assert!(!claim.summary.contains("已恢复"), "活动告警批次不得渲染恢复提示：{}", claim.summary);

    // 领取之后、入队确认之前提交的静默，不能被入队冷却覆盖成更早的时间。
    let silence = auction_application::operations::OperationsAlertSilenceCommand::new(
        Uuid::new_v4(),
        fingerprint,
        7 * 24 * 3_600,
        "operator",
        "planned maintenance",
    )
    .expect("构造静默命令");
    let silenced_until = store
        .silence_alert(silence)
        .await
        .expect("写入静默")
        .silenced_until
        .expect("静默结果必须带截止时间");
    store.mark_enqueued(claim, Duration::from_secs(1_800)).await.expect("确认入队");
    let (stored_silenced_until, next_notify_at): (
        Option<chrono::DateTime<Utc>>,
        Option<chrono::DateTime<Utc>>,
    ) = sqlx::query_as(
        "SELECT silenced_until, next_notify_at
         FROM public.operations_alert_signals WHERE fingerprint = $1",
    )
    .bind(fingerprint.as_slice())
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取静默与重复通知时间");
    assert_eq!(
        stored_silenced_until.map(|at| at.timestamp_micros()),
        Some(silenced_until.timestamp_micros()),
        "静默截止时间不得被入队确认改写"
    );
    let next_notify_at = next_notify_at.expect("重复通知时间必须保留");
    let silenced_until_db = stored_silenced_until.expect("静默截止时间必须保留");
    assert!(
        next_notify_at >= silenced_until_db,
        "入队冷却 {next_notify_at} 不得早于静默截止 {silenced_until_db}"
    );
    assert!(
        store
            .claim_due_batch(request(silenced_until_db - chrono::Duration::seconds(60)))
            .await
            .expect("静默窗口内领取")
            .is_none(),
        "静默期间不得再次领取同一告警"
    );
    assert!(
        store
            .claim_due_batch(request(silenced_until_db + chrono::Duration::seconds(1)))
            .await
            .expect("静默到期后领取")
            .is_some(),
        "静默到期后应恢复重复通知"
    );
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_operations_alert_webhook_retry_exhausted_is_terminal_failure() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let mut fingerprint = [0_u8; 32];
    fingerprint[..11].copy_from_slice(b"webhook-exh");
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, next_webhook_at, count_value, age_seconds, state_version)
         VALUES ($1, 'service_missing', 'worker', 'process', true, now(), now(),
                 now() + interval '30 minutes', now() - interval '1 second', 1, 1, 1)",
    )
    .bind(fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("写入耗尽 signal");
    let now = Utc::now();
    let dispatch = store
        .claim_due_webhook(auction_application::operations::OperationsAlertWebhookClaimRequest {
            now,
            lease_until: now + chrono::Duration::seconds(120),
            template: auction_application::operations::DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE
                .to_owned(),
        })
        .await
        .expect("领取耗尽 signal")
        .expect("到期");
    store
        .finish_webhook(
            dispatch,
            auction_application::operations::OperationsAlertWebhookFinish {
                outcome:
                    auction_application::operations::OperationsAlertWebhookOutcome::RetryExhausted,
                next_webhook_at: None,
            },
        )
        .await
        .expect("写入耗尽终态");
    let (status, failure_code): (String, Option<String>) = sqlx::query_as(
        "SELECT status, failure_code FROM public.operations_alert_webhook_deliveries
         WHERE fingerprint = $1",
    )
    .bind(fingerprint.as_slice())
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取耗尽终态");
    assert_eq!(status, "failed");
    assert_eq!(failure_code.as_deref(), Some("retry_exhausted"));
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_mail_poller_paused_since_ignores_forced_poll_updates() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let paused_at = Utc::now() - chrono::Duration::hours(2);
    let last_poll_at = Utc::now() - chrono::Duration::minutes(5);
    sqlx::query(
        "INSERT INTO public.mail_poll_state
            (mailbox_key, last_poll_at, next_poll_at, consecutive_failures,
             paused, state_version, updated_at)
         VALUES ('main-mailbox', $1, $2, 0, true, 1, $3)",
    )
    .bind(last_poll_at)
    .bind(last_poll_at + chrono::Duration::minutes(1))
    .bind(Utc::now())
    .execute(store.postgres_pool())
    .await
    .expect("写入暂停轮询器");
    sqlx::query(
        "INSERT INTO public.mail_admin_commands
            (command_id, command_kind, request_digest, mailbox_key, result_payload, created_at)
         VALUES ($1, 'set_polling_paused', $2, 'main-mailbox',
                 '{\"paused\":true}'::jsonb, $3)",
    )
    .bind(Uuid::new_v4())
    .bind([7_u8; 32].as_slice())
    .bind(paused_at)
    .execute(store.postgres_pool())
    .await
    .expect("写入暂停命令");
    let snapshot = store.dashboard_snapshot().await.expect("读取邮件 dashboard");
    assert_eq!(
        snapshot.pollers[0].paused_since.expect("暂停时间").timestamp_micros(),
        paused_at.timestamp_micros()
    );

    // 强制轮询可更新 updated_at，但不得改写暂停起始时间。
    sqlx::query(
        "UPDATE public.mail_poll_state SET updated_at = $1 WHERE mailbox_key = 'main-mailbox'",
    )
    .bind(Utc::now())
    .execute(store.postgres_pool())
    .await
    .expect("模拟强制轮询更新时间");
    let snapshot = store.dashboard_snapshot().await.expect("再次读取邮件 dashboard");
    assert_eq!(
        snapshot.pollers[0].paused_since.expect("暂停时间").timestamp_micros(),
        paused_at.timestamp_micros()
    );
    store.postgres_pool().close().await;
}

/// 上一活动周期遗留的 manual_reconcile 只保留未知结果，不能永久阻断同一 fingerprint 的新事件。
#[tokio::test]
async fn postgres_webhook_generation_allows_new_incident_after_manual_reconcile() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let mut fingerprint = [0_u8; 32];
    fingerprint[..11].copy_from_slice(b"webhook-gen");
    sqlx::query(
        "INSERT INTO public.operations_alert_signals
            (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
             next_notify_at, next_webhook_at, count_value, age_seconds, state_version)
         VALUES ($1, 'service_missing', 'worker', 'process', true, now(), now(),
                 now() + interval '30 minutes', now() - interval '1 second', 1, 4, 1)",
    )
    .bind(fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("写入首个活动周期 signal");
    let request = |now: chrono::DateTime<Utc>| {
        auction_application::operations::OperationsAlertWebhookClaimRequest {
            now,
            lease_until: now + chrono::Duration::seconds(120),
            template: "{\"format\":\"auction-operations-alert.v1\"}".to_owned(),
        }
    };
    let now = Utc::now();
    let first = store
        .claim_due_webhook(request(now))
        .await
        .expect("领取首个 webhook")
        .expect("首个周期应到期");
    // 模拟 snapshot/observe：先恢复，再重新出现；generation 只在 inactive→active 时递增。
    sqlx::query(
        "UPDATE public.operations_alert_signals
         SET active = false, resolved_at = now(), next_notify_at = NULL,
             next_webhook_at = NULL
         WHERE fingerprint = $1",
    )
    .bind(fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("恢复首个事件");
    sqlx::query(
        "UPDATE public.operations_alert_signals
         SET active = true, resolved_at = NULL,
             next_notify_at = now() + interval '30 minutes',
             next_webhook_at = now() - interval '1 second',
             webhook_generation = webhook_generation + 1
         WHERE fingerprint = $1",
    )
    .bind(fingerprint.as_slice())
    .execute(store.postgres_pool())
    .await
    .expect("重新激活事件");

    // 先重激活新周期，再完成旧周期投递；旧结果不能改写新周期的 next_webhook_at。
    store
        .finish_webhook(
            first.clone(),
            auction_application::operations::OperationsAlertWebhookFinish {
                outcome: auction_application::operations::OperationsAlertWebhookOutcome::Unknown,
                next_webhook_at: None,
            },
        )
        .await
        .expect("把旧周期结果标为未知");
    let next_webhook_at: Option<chrono::DateTime<Utc>> = sqlx::query_scalar(
        "SELECT next_webhook_at FROM public.operations_alert_signals WHERE fingerprint = $1",
    )
    .bind(fingerprint.as_slice())
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取新周期 webhook 冷却");
    assert!(next_webhook_at.is_some(), "旧周期完成后不得清空新周期的 next_webhook_at");

    let second = store
        .claim_due_webhook(request(Utc::now()))
        .await
        .expect("领取新周期 webhook")
        .expect("上一周期 manual_reconcile 不得阻断新周期");
    assert_ne!(first.delivery_id, second.delivery_id);
    let generations: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, signal_generation FROM public.operations_alert_webhook_deliveries
         ORDER BY signal_generation",
    )
    .fetch_all(store.postgres_pool())
    .await
    .expect("读取两个周期的投递");
    assert_eq!(generations, vec![("manual_reconcile".to_owned(), 1), ("sending".to_owned(), 2)]);
    store.postgres_pool().close().await;
}
