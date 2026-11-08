//! 出价被超提醒在 lot 有效截止后的预检退役（W1.7，决策 14）。
//!
//! 只经公开存储端口驱动：MoogleMail 走 `DeliveryWorkStore`，论坛 PM 走
//! `LotWatchPmWorkStore`。覆盖截止前可发送、截止后退役、发送围栏拒绝与已经 sending
//! 的结果不被改写。仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[allow(dead_code)]
use crate::support;

use auction_application::delivery::{DeliveryClaimExclusions, DeliveryWorkStore};
use auction_application::lot_watch_pm::{LotWatchPmClaimRequest, LotWatchPmWorkStore};
use auction_application::lot_watch_reminders::LotWatchReminderStore;
use auction_application::mail::MailNotificationTemplate;
use auction_application::public::{
    LotWatchPort, OutbidNotice, SetLotWatchCommand, lot_watch_notification,
    lot_watch_pm_notification,
};
use auction_domain::CommandId;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::{Acquire, Row};
use uuid::Uuid;

const WORKER: &str = "lot-watch-deadline-test";
const PM_SENDER: &str = "424242";

struct WatchFixture {
    lot_id: Uuid,
    notification_id: Uuid,
    delivery_id: Option<Uuid>,
}

/// 截止前的有效提醒最少字段：world/lot 绑定、被超越用户、当前领先者与提醒事实。
async fn seed_watch(
    store: &auction_storage::DatabaseStore,
    seed: &str,
    world: &str,
    channel: &str,
    deadline: DateTime<Utc>,
) -> WatchFixture {
    let pool = store.postgres_pool();
    let watcher_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-watcher").as_bytes());
    let leader_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-leader").as_bytes());
    for (user_id, username) in [(watcher_id, format!("{seed}-w")), (leader_id, format!("{seed}-l"))]
    {
        // public_key_hash 唯一；用稳定 UUID 字节派生，避免不同测试用户互相冲突。
        let mut public_key_hash = [0_u8; 32];
        public_key_hash[..16].copy_from_slice(
            Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{username}-key").as_bytes()).as_bytes(),
        );
        public_key_hash[16..].copy_from_slice(
            Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{username}-salt").as_bytes()).as_bytes(),
        );
        sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(username)
            .bind(public_key_hash.as_slice())
            .execute(pool)
            .await
            .expect("写入测试用户");
    }

    let auction_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-auction").as_bytes());
    let equipment_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-equipment").as_bytes());
    let lot_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-lot").as_bytes());
    let opened = deadline - Duration::hours(2);
    // 新场次冻结策略 v5；提醒退役只依赖 lot 有效截止，不改写任何策略字段。
    let policy = json!({
        "schema_version": 5,
        "bidding_version": 2,
        "start_price": 50000,
        "bid_increment_ppm": 0,
        "minimum_increment": 1000,
        "soft_close_window_seconds": 900,
        "soft_close_extension_seconds": 900,
        "proxy_enabled": true,
        "proxy_default_mode": "fast",
        "proxy_slow_interval_seconds": 600,
        "proxy_max_outbid_uses": 0,
        "final_phase_lead_seconds": 900,
        "scheduled_end_at": deadline,
        "settlement": {
            "version": 1,
            "mode": "hold_curve_v1",
            "compensation_pool_enabled": true,
            "anchors": []
        },
        "recovery": {"version": 1, "mode": "remind_v1"}
    });
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, policy_snapshot)
         VALUES ($1, $2, 1, $3, 'live', $4, $4, $5, $5, $6)",
    )
    .bind(auction_id)
    .bind(world)
    .bind(format!("{seed} auction"))
    .bind(opened)
    .bind(deadline)
    .bind(&policy)
    .execute(pool)
    .await
    .expect("写入测试场次");

    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, collected_at)
         VALUES ($1, $5, $2, $3, 'weapon', $4)",
    )
    .bind(equipment_id)
    .bind(format!("{seed}-eid"))
    .bind(format!("{seed} equipment"))
    .bind(opened)
    .bind(world)
    .execute(pool)
    .await
    .expect("写入测试装备");

    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             leader_user_id, effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'live', 50000, 120000, $4, $5, 'weapon', 1)",
    )
    .bind(lot_id)
    .bind(auction_id)
    .bind(equipment_id)
    .bind(leader_id)
    .bind(deadline)
    .execute(pool)
    .await
    .expect("写入测试 lot");

    sqlx::query(
        "INSERT INTO public.lot_watches (user_id, lot_id, watched, channel, version)
         VALUES ($1, $2, true, 'mm', 1)",
    )
    .bind(watcher_id)
    .bind(lot_id)
    .execute(pool)
    .await
    .expect("写入测试标记");

    // 迁移 000090 起 lot 创建事务会写入初始领先周期（cycle = 首次状态版本）；
    // 测试需要显式的第 2 周期，因此覆盖同一行而不是再插入一行。
    sqlx::query(
        "INSERT INTO public.lot_watch_leadership (lot_id, leader_user_id, cycle)
         VALUES ($1, $2, 2)
         ON CONFLICT (lot_id) DO UPDATE
         SET leader_user_id = EXCLUDED.leader_user_id, cycle = EXCLUDED.cycle",
    )
    .bind(lot_id)
    .bind(leader_id)
    .execute(pool)
    .await
    .expect("写入领先周期");

    // 被超越的出价事实：只用于冻结通知来源，不参与本次退役判定。
    let bid_command = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-command").as_bytes());
    let bid_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-bid").as_bytes());
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'place_bid', $2, $3, 'accepted')",
    )
    .bind(bid_command)
    .bind(auction_id)
    .bind([2_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入测试命令");
    sqlx::query(
        "INSERT INTO public.bids
            (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
             effective_end_at, accepted_state_version)
         VALUES ($1, $2, 1, $3, $4, 120000, 'ordinary', $5, 2)",
    )
    .bind(bid_id)
    .bind(bid_command)
    .bind(lot_id)
    .bind(leader_id)
    .bind(deadline)
    .execute(pool)
    .await
    .expect("写入测试出价");

    if channel == "pm" {
        let notification = lot_watch_pm_notification(
            lot_id,
            watcher_id,
            1,
            120000,
            OutbidNotice {
                lot_code: "Clo01",
                item_name: "Phase Robe of Niflheim",
                deadline: Some(deadline),
                site_url: Some("https://auction.example.com"),
            },
            &Default::default(),
            Some("9001".to_owned()),
        )
        .expect("构造 PM 提醒");
        sqlx::query(
            "INSERT INTO public.lot_watch_notifications
                (id, user_id, lot_id, leadership_cycle, displaced_bid_id, watch_version,
                 command_id, request_digest, recipient, payload, created_at, channel,
                 preference_version)
             VALUES ($1, $2, $3, 1, $4, 1, $1, $5, $6, $7, $8, 'pm', 1)",
        )
        .bind(notification.notification_id)
        .bind(watcher_id)
        .bind(lot_id)
        .bind(bid_id)
        .bind(notification.digest.as_bytes().as_slice())
        .bind(&notification.recipient)
        .bind(&notification.payload)
        .bind(Utc::now())
        .execute(pool)
        .await
        .expect("写入 PM 提醒来源");
        sqlx::query(
            "INSERT INTO public.lot_watch_pm_deliveries (notification_id, status) VALUES ($1, 'pending')",
        )
        .bind(notification.notification_id)
        .execute(pool)
        .await
        .expect("写入 PM 队列");
        return WatchFixture {
            lot_id,
            notification_id: notification.notification_id,
            delivery_id: None,
        };
    }

    let command = lot_watch_notification(
        lot_id,
        watcher_id,
        1,
        120000,
        OutbidNotice {
            lot_code: "Clo01",
            item_name: "Phase Robe of Niflheim",
            deadline: Some(deadline),
            site_url: Some("https://auction.example.com"),
        },
        &Default::default(),
        "main".to_owned(),
        format!("{seed}-w"),
    )
    .expect("构造 MoogleMail 提醒");
    let notification_id = command.command_id.as_uuid();
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": command.world_id,
        "external_mail_id": command.external_mail_id,
        "template": command.template.map(MailNotificationTemplate::as_str),
        "result_code": command.result_code,
        "user_key_user_id": command.user_key_user_id,
        "credits": command.credits,
        "cod": command.cod,
        "attachment_ids": command.attachment_ids,
        "body": command.notification_body,
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, $3, 'lot_watch', $4, $5, $6, 'pending')",
    )
    .bind(notification_id)
    .bind(command.command_id.as_uuid())
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(&command.origin_id)
    .bind(&command.recipient)
    .bind(&payload)
    .execute(pool)
    .await
    .expect("写入提醒投递");
    sqlx::query(
        "INSERT INTO public.lot_watch_notifications
            (id, user_id, lot_id, leadership_cycle, displaced_bid_id, watch_version, delivery_id,
             command_id, request_digest, recipient, payload, created_at, channel,
             preference_version)
         VALUES ($1, $2, $3, 1, $4, 1, $5, $1, $6, $7, $8, $9, 'mm', 1)",
    )
    .bind(notification_id)
    .bind(watcher_id)
    .bind(lot_id)
    .bind(bid_id)
    .bind(notification_id)
    .bind(command.request_hash.as_bytes().as_slice())
    .bind(&command.recipient)
    .bind(&payload)
    .bind(Utc::now())
    .execute(pool)
    .await
    .expect("写入提醒来源");
    WatchFixture { lot_id, notification_id, delivery_id: Some(notification_id) }
}

async fn claim_mail(
    store: &auction_storage::DatabaseStore,
) -> Vec<auction_application::delivery::DeliveryWorkClaim> {
    let now = Utc::now();
    store
        .claim_due(
            WORKER.to_owned(),
            now,
            now + Duration::minutes(2),
            std::time::Duration::from_secs(60),
            DeliveryClaimExclusions::default(),
            10,
        )
        .await
        .expect("领取提醒投递")
}

async fn mail_state(
    store: &auction_storage::DatabaseStore,
    delivery_id: Uuid,
) -> (String, Option<String>, Option<DateTime<Utc>>, i64) {
    let row = sqlx::query(
        "SELECT status, failure_code, sending_at, fencing_version FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取提醒投递状态");
    (row.get("status"), row.get("failure_code"), row.get("sending_at"), row.get("fencing_version"))
}

async fn pm_state(
    store: &auction_storage::DatabaseStore,
    notification_id: Uuid,
) -> (String, Option<String>, Option<DateTime<Utc>>) {
    let row = sqlx::query(
        "SELECT status, failure_code, sending_at FROM public.lot_watch_pm_deliveries
         WHERE notification_id = $1",
    )
    .bind(notification_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取 PM 队列状态");
    (row.get("status"), row.get("failure_code"), row.get("sending_at"))
}

async fn pm_claim(
    store: &auction_storage::DatabaseStore,
) -> Option<auction_application::lot_watch_pm::LotWatchPmClaim> {
    store
        .claim(LotWatchPmClaimRequest {
            worker_id: WORKER.to_owned(),
            lease_seconds: 120,
            reconcile: false,
            available_worlds: vec![
                auction_domain::SettlementWorld::Main,
                auction_domain::SettlementWorld::Isekai,
            ],
        })
        .await
        .expect("领取 PM 提醒")
}

async fn expire_lot(store: &auction_storage::DatabaseStore, lot_id: Uuid) {
    sqlx::query("UPDATE public.lots SET effective_end_at = clock_timestamp() - interval '1 second' WHERE id = $1")
        .bind(lot_id)
        .execute(store.postgres_pool())
        .await
        .expect("把测试 lot 置为已过有效截止");
}

#[tokio::test]
async fn postgres_lot_watch_mail_reminder_is_claimable_before_the_deadline() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let fixture =
        seed_watch(&store, "mail-before", "main", "mm", Utc::now() + Duration::minutes(5)).await;
    let delivery_id = fixture.delivery_id.expect("MoogleMail 提醒有投递");

    let claims = claim_mail(&store).await;
    assert_eq!(claims.len(), 1, "截止前的提醒应可领取");
    assert_eq!(claims[0].delivery_id, delivery_id);

    let (status, failure, sending_at, fencing) = mail_state(&store, delivery_id).await;
    assert_eq!(status, "claimed");
    assert!(failure.is_none());
    assert!(sending_at.is_none());
    assert!(fencing > 0);
}

/// 发送资格必须看事务内权威的 `lots.leader_user_id`；延迟更新的领先投影不能把
/// 已经重新领先的收件人误判为被超越。
#[tokio::test]
async fn postgres_lot_watch_mail_gate_uses_authoritative_lot_leader() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let fixture =
        seed_watch(&store, "mail-stale-leader", "main", "mm", Utc::now() + Duration::minutes(5))
            .await;
    let pool = store.postgres_pool();
    let watcher: Uuid =
        sqlx::query_scalar("SELECT user_id FROM public.lot_watch_notifications WHERE id = $1")
            .bind(fixture.notification_id)
            .fetch_one(pool)
            .await
            .expect("读取提醒收件人");
    sqlx::query("UPDATE public.lot_watch_leadership SET leader_user_id = $2 WHERE lot_id = $1")
        .bind(fixture.lot_id)
        .bind(watcher)
        .execute(pool)
        .await
        .expect("模拟滞后的领先投影");

    let claims = claim_mail(&store).await;
    assert_eq!(claims.len(), 1, "权威 lot 领先者不是收件人时提醒仍应可领取");

    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_lot_watch_mail_reminder_retires_after_the_deadline_before_any_send() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let fixture =
        seed_watch(&store, "mail-after", "main", "mm", Utc::now() - Duration::seconds(1)).await;
    let delivery_id = fixture.delivery_id.expect("MoogleMail 提醒有投递");

    let claims = claim_mail(&store).await;
    assert!(claims.is_empty(), "截止后的提醒不能再被领取");

    let (status, failure, sending_at, fencing) = mail_state(&store, delivery_id).await;
    assert_eq!(status, "failed", "沿用预检退役语义记为未发送失败");
    assert_eq!(failure.as_deref(), Some("lot_watch_invalidated"));
    assert!(sending_at.is_none(), "退役不得伪造发送围栏");
    assert!(fencing > 0);
    let metadata: serde_json::Value =
        sqlx::query_scalar("SELECT result_metadata FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取退役元数据");
    assert_eq!(metadata["preflight_blocked"], json!(true));
    assert_eq!(metadata["outcome_unknown"], json!(false));
}

#[tokio::test]
async fn postgres_lot_watch_mail_reminder_fence_rejects_a_claim_that_outlived_the_deadline() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let fixture =
        seed_watch(&store, "mail-fence", "main", "mm", Utc::now() + Duration::minutes(5)).await;
    let delivery_id = fixture.delivery_id.expect("MoogleMail 提醒有投递");
    let claims = claim_mail(&store).await;
    assert_eq!(claims.len(), 1);

    // 领取发生在截止前，围栏发生在截止后：绝不允许开始发送。
    expire_lot(&store, fixture.lot_id).await;
    let fence = DeliveryWorkStore::start_sending(&store, claims[0].clone(), Utc::now()).await;
    assert!(fence.is_err(), "截止后 sending 围栏必须拒绝");

    let (status, failure, sending_at, _) = mail_state(&store, delivery_id).await;
    assert_eq!(status, "claimed", "围栏拒绝不改写未发送的领取状态");
    assert!(failure.is_none());
    assert!(sending_at.is_none());

    // 下一轮预检按同一退役语义收口，不再重试。
    let next = claim_mail(&store).await;
    assert!(next.is_empty());
    let (status, failure, sending_at, _) = mail_state(&store, delivery_id).await;
    assert_eq!(status, "failed");
    assert_eq!(failure.as_deref(), Some("lot_watch_invalidated"));
    assert!(sending_at.is_none());
}

#[tokio::test]
async fn postgres_lot_watch_mail_reminder_keeps_an_already_fenced_send() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let fixture =
        seed_watch(&store, "mail-sent", "main", "mm", Utc::now() - Duration::seconds(1)).await;
    let delivery_id = fixture.delivery_id.expect("MoogleMail 提醒有投递");
    let sending_at = Utc::now() - Duration::seconds(30);
    sqlx::query(
        "UPDATE public.deliveries SET status = 'sending', sending_at = $2, lease_token = $3,
            lease_expires_at = $4, attempt_count = 1
         WHERE id = $1",
    )
    .bind(delivery_id)
    .bind(sending_at)
    .bind(Uuid::new_v4())
    .bind(Utc::now() + Duration::minutes(5))
    .execute(store.postgres_pool())
    .await
    .expect("模拟已进入发送围栏的提醒");

    let claims = claim_mail(&store).await;
    assert!(claims.is_empty(), "已发送的提醒不能被重新领取或退役");

    let (status, failure, stored_sending_at, _) = mail_state(&store, delivery_id).await;
    assert_eq!(status, "sending");
    assert!(failure.is_none());
    assert_eq!(
        stored_sending_at.map(|value| value.timestamp_micros()),
        Some(sending_at.timestamp_micros()),
        "已发送的提醒时间不被退役改写"
    );
}

#[tokio::test]
async fn postgres_lot_watch_pm_reminder_parity_before_and_after_the_deadline() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");

    // 截止前可领取并可开始发送。
    let before =
        seed_watch(&store, "pm-before", "main", "pm", Utc::now() + Duration::minutes(5)).await;
    let claim = pm_claim(&store).await.expect("截止前的 PM 提醒应可领取");
    assert_eq!(claim.message.notification_id, before.notification_id);
    let started = LotWatchPmWorkStore::start_sending(&store, claim, PM_SENDER.to_owned())
        .await
        .expect("开始 PM 发送");
    assert!(started, "截止前 PM 发送围栏应通过");
    let (status, _, sending_at) = pm_state(&store, before.notification_id).await;
    assert_eq!(status, "sending");
    assert!(sending_at.is_some());

    // 截止后尚未发送的 PM 按同一预检语义退役为 suppressed/source_invalid。
    let after =
        seed_watch(&store, "pm-after", "isekai", "pm", Utc::now() - Duration::seconds(1)).await;
    assert!(pm_claim(&store).await.is_none(), "截止后的 PM 提醒不能再被领取");
    let (status, failure, sending_at) = pm_state(&store, after.notification_id).await;
    assert_eq!(status, "suppressed");
    assert_eq!(failure.as_deref(), Some("source_invalid"));
    assert!(sending_at.is_none(), "退役不得伪造 PM 发送围栏");
}

#[tokio::test]
async fn postgres_lot_watch_pm_reminder_fence_and_fenced_send_ignore_the_expired_deadline() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");

    // 领取在截止前，发送围栏在截止后：拒绝并退役，不发送。
    let fixture =
        seed_watch(&store, "pm-fence", "main", "pm", Utc::now() + Duration::minutes(5)).await;
    let claim = pm_claim(&store).await.expect("截止前 PM 可领取");
    expire_lot(&store, fixture.lot_id).await;
    let started = LotWatchPmWorkStore::start_sending(&store, claim, PM_SENDER.to_owned())
        .await
        .expect("PM 围栏调用成功");
    assert!(!started, "截止后的 PM 发送围栏必须拒绝");
    let (status, failure, sending_at) = pm_state(&store, fixture.notification_id).await;
    assert_eq!(status, "suppressed");
    assert_eq!(failure.as_deref(), Some("source_invalid"));
    assert!(sending_at.is_none());

    // 已经 sending 的 PM 结果不受退役影响，也不回发送队列。
    let fenced =
        seed_watch(&store, "pm-sent", "isekai", "pm", Utc::now() - Duration::seconds(1)).await;
    let sending_at = Utc::now() - Duration::seconds(30);
    sqlx::query(
        "UPDATE public.lot_watch_pm_deliveries SET status = 'sending', sending_at = $2,
            sender_forum_account_id = $3, lease_token = $4, lease_expires_at = $5,
            lease_kind = 'send', worker_id = $6, attempt_count = 1
         WHERE notification_id = $1",
    )
    .bind(fenced.notification_id)
    .bind(sending_at)
    .bind(PM_SENDER)
    .bind(Uuid::new_v4())
    .bind(Utc::now() + Duration::minutes(5))
    .bind(WORKER)
    .execute(store.postgres_pool())
    .await
    .expect("模拟已进入 PM 发送围栏的提醒");

    assert!(pm_claim(&store).await.is_none());
    let (status, failure, stored_sending_at) = pm_state(&store, fenced.notification_id).await;
    assert_eq!(status, "sending");
    assert!(failure.is_none());
    assert_eq!(
        stored_sending_at.map(|value| value.timestamp_micros()),
        Some(sending_at.timestamp_micros()),
        "已发送的提醒时间不被退役改写"
    );
}

/// PM 发送围栏必须看事务内权威的 `lots.leader_user_id`；提交后才更新的领先投影不能
/// 授权一条收件人早已重新领先的历史提醒。
#[tokio::test]
async fn postgres_lot_watch_pm_gate_uses_authoritative_lot_leader() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let fixture =
        seed_watch(&store, "pm-stale-leader", "main", "pm", Utc::now() + Duration::minutes(5))
            .await;
    let pool = store.postgres_pool();
    let watcher: Uuid =
        sqlx::query_scalar("SELECT user_id FROM public.lot_watch_notifications WHERE id = $1")
            .bind(fixture.notification_id)
            .fetch_one(pool)
            .await
            .expect("读取提醒收件人");
    // 收件人已重新领先：权威 lot 行在出价事务内更新，领先投影仍停在旧领先者。
    sqlx::query("UPDATE public.lots SET leader_user_id = $2 WHERE id = $1")
        .bind(fixture.lot_id)
        .bind(watcher)
        .execute(pool)
        .await
        .expect("更新权威 lot 领先者");

    let claim = pm_claim(&store).await.expect("PM 提醒仍可被领取");
    assert_eq!(claim.message.notification_id, fixture.notification_id);
    let started = LotWatchPmWorkStore::start_sending(&store, claim, PM_SENDER.to_owned())
        .await
        .expect("PM 围栏调用成功");
    assert!(!started, "收件人已重新领先时 PM 发送围栏必须拒绝");
    let (status, failure, sending_at) = pm_state(&store, fixture.notification_id).await;
    assert_eq!(status, "suppressed");
    assert_eq!(failure.as_deref(), Some("source_invalid"));
    assert!(sending_at.is_none(), "退役不得伪造 PM 发送围栏");
    pool.close().await;
}

/// outbox 必须与显式取消共享 lot 行锁：取消先提交时不得再创建有效提醒，
/// 否则用户重新标记后发送围栏会接受这条已撤销的历史提醒。
#[tokio::test]
async fn postgres_lot_watch_outbox_serializes_with_explicit_unwatch() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let fixture =
        seed_watch(&store, "outbox-unwatch", "main", "mm", Utc::now() + Duration::minutes(5)).await;
    let pool = store.postgres_pool().clone();
    let watcher: Uuid =
        sqlx::query_scalar("SELECT user_id FROM public.lot_watch_notifications WHERE id = $1")
            .bind(fixture.notification_id)
            .fetch_one(&pool)
            .await
            .expect("读取提醒收件人");
    let auction_id: Uuid = sqlx::query_scalar("SELECT auction_id FROM public.lots WHERE id = $1")
        .bind(fixture.lot_id)
        .fetch_one(&pool)
        .await
        .expect("读取测试场次");

    // 收件人重新领先后又被新的挑战者反超：outbox 会尝试为收件人再创建一条被超提醒。
    let challenger = Uuid::new_v4();
    let mut public_key_hash = [0_u8; 32];
    public_key_hash[..16].copy_from_slice(challenger.as_bytes());
    public_key_hash[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(challenger)
        .bind(format!("challenger-{challenger}"))
        .bind(public_key_hash.as_slice())
        .execute(&pool)
        .await
        .expect("写入挑战者");
    sqlx::query("UPDATE public.lot_watch_leadership SET leader_user_id = $2 WHERE lot_id = $1")
        .bind(fixture.lot_id)
        .bind(watcher)
        .execute(&pool)
        .await
        .expect("收件人重新领先");
    sqlx::query("UPDATE public.lots SET leader_user_id = $2 WHERE id = $1")
        .bind(fixture.lot_id)
        .bind(watcher)
        .execute(&pool)
        .await
        .expect("同步权威领先者");

    let command_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'place_bid', $2, $3, 'accepted')",
    )
    .bind(command_id)
    .bind(auction_id)
    .bind([9_u8; 32].as_slice())
    .execute(&pool)
    .await
    .expect("写入挑战命令");
    let bid_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.bids
            (id, command_id, fact_index, lot_id, bidder_user_id, amount, mechanism,
             effective_end_at, accepted_state_version)
         VALUES ($1, $2, 1, $3, $4, 121000, 'ordinary', $5, 3)",
    )
    .bind(bid_id)
    .bind(command_id)
    .bind(fixture.lot_id)
    .bind(challenger)
    .bind(Utc::now() + Duration::minutes(5))
    .execute(&pool)
    .await
    .expect("写入挑战出价");
    sqlx::query(
        "INSERT INTO public.lot_watch_events
            (event_id, event_kind, lot_id, user_id, bid_id, state_version,
             occurred_at, next_attempt_at)
         VALUES ($1, 'bid_accepted', $2, $3, $4, 3, clock_timestamp(), clock_timestamp())",
    )
    .bind(Uuid::new_v4())
    .bind(fixture.lot_id)
    .bind(challenger)
    .bind(bid_id)
    .execute(&pool)
    .await
    .expect("写入待处理标记事件");

    // 模拟显式取消命令：先取 lot 行锁，再写标记与撤销，提交前保持未决。
    let mut connection = pool.acquire().await.expect("取得取消事务连接");
    let mut unwatch = connection.begin().await.expect("开始取消事务");
    sqlx::query("SELECT id FROM public.lots WHERE id = $1 FOR UPDATE")
        .bind(fixture.lot_id)
        .execute(&mut *unwatch)
        .await
        .expect("锁定 lot 行");
    sqlx::query(
        "UPDATE public.lot_watches SET watched = false, version = version + 1,
             updated_at = clock_timestamp()
         WHERE user_id = $1 AND lot_id = $2",
    )
    .bind(watcher)
    .bind(fixture.lot_id)
    .execute(&mut *unwatch)
    .await
    .expect("写入取消标记");
    sqlx::query(
        "UPDATE public.lot_watch_notifications
         SET invalidated_at = clock_timestamp(), invalidation_reason = 'unwatched'
         WHERE user_id = $1 AND lot_id = $2 AND invalidated_at IS NULL",
    )
    .bind(watcher)
    .bind(fixture.lot_id)
    .execute(&mut *unwatch)
    .await
    .expect("撤销既有提醒");

    let worker_store = store.clone();
    let processing =
        tokio::spawn(async move { worker_store.process_due(WORKER.to_owned(), 60, 10).await });
    // 事件领取与处理不在同一事务：等到领取提交后，处理必须阻塞在 lot 行锁上。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status: String =
            sqlx::query_scalar("SELECT status FROM public.lot_watch_events WHERE lot_id = $1")
                .bind(fixture.lot_id)
                .fetch_one(&pool)
                .await
                .expect("读取事件领取状态");
        if status == "processing" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "事件未被领取");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(!processing.is_finished(), "outbox 处理必须等待与显式取消共享的 lot 行锁");

    unwatch.commit().await.expect("提交取消事务");
    drop(connection);
    let batch = processing.await.expect("处理任务不得 panic").expect("标记事件处理成功");
    assert_eq!(batch.completed, 1);

    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.lot_watch_notifications WHERE user_id = $1 AND lot_id = $2",
    )
    .bind(watcher)
    .bind(fixture.lot_id)
    .fetch_one(&pool)
    .await
    .expect("读取提醒条数");
    assert_eq!(total, 1, "取消提交后不得再插入新的提醒");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.lot_watch_notifications
         WHERE user_id = $1 AND lot_id = $2 AND invalidated_at IS NULL",
    )
    .bind(watcher)
    .bind(fixture.lot_id)
    .fetch_one(&pool)
    .await
    .expect("读取有效提醒条数");
    assert_eq!(live, 0, "已取消的标记不得留下可发送的提醒");
    pool.close().await;
}

#[tokio::test]
async fn postgres_unwatch_retires_pending_slow_proxy_reminder() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let deadline = Utc::now() + Duration::hours(1);
    let seed = "pending-unwatch";
    let fixture = seed_watch(&store, seed, "main", "mm", deadline).await;
    let watcher = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-watcher").as_bytes());
    let bid_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-bid").as_bytes());
    let pending_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}-pending").as_bytes());
    sqlx::query(
        "INSERT INTO public.lot_watch_pending_notifications
            (id, user_id, lot_id, leadership_cycle, displaced_bid_id, watch_version,
             preference_version, channel, recipient, amount, status, created_at)
         VALUES ($1, $2, $3, 2, $4, 1, 1, 'mm', $5, 120000, 'pending', now())",
    )
    .bind(pending_id)
    .bind(watcher)
    .bind(fixture.lot_id)
    .bind(bid_id)
    .bind(format!("{seed}-w"))
    .execute(store.postgres_pool())
    .await
    .expect("写入待定提醒");

    store
        .set(SetLotWatchCommand {
            command_id: CommandId::new(),
            user_id: watcher,
            lot_id: fixture.lot_id,
            watched: false,
            channel: "mm".to_owned(),
            expected_version: 1,
        })
        .await
        .expect("取消标记");
    let (status, reason): (String, Option<String>) = sqlx::query_as(
        "SELECT status, resolution_reason FROM public.lot_watch_pending_notifications
         WHERE id = $1",
    )
    .bind(pending_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取待定提醒状态");
    assert_eq!(status, "retired");
    assert_eq!(reason.as_deref(), Some("watch_removed"));
    store.postgres_pool().close().await;
}
