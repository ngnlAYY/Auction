//! “已寄出、等待领取”在真实 PostgreSQL 18 schema 上的读侧口径，全部以运行账号执行：
//! - 告警/看板履约水位把已寄出的 Send 义务从积压拆到等待领取，单列最近一次读取失败；
//!   COD 仍按既有口径留在原分类，人工核对与尚未寄出的义务不变。
//!
//! 入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration as StdDuration;

use auction_application::operations::{
    OperationsAlertKind, OperationsAlertPolicy, OperationsAlertSource, OperationsAlertSourceReader,
    evaluate_snapshot,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

const SEED: &str = "collection-wait";
const SELLER: &str = "collection-seller";

/// 一场已结算的 main 场次与其命令；义务的 origin 命令指向它。
struct Auction {
    id: Uuid,
    command_id: Uuid,
    winner: Uuid,
}

/// 一个 lot 及其装备。
struct Lot {
    id: Uuid,
    equipment_id: Uuid,
}

async fn seed_auction(pool: &PgPool, now: DateTime<Utc>) -> Auction {
    let auction = Auction {
        id: stable_id(SEED, "auction"),
        command_id: stable_id(SEED, "command"),
        winner: stable_id(SEED, "winner"),
    };
    seed_user(pool, auction.winner, "collection-winner").await;
    let ended = now - Duration::days(10);
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 4, 'collection wait', 'settled', $2, $2, $2, $2, $2, $2, $3)",
    )
    .bind(auction.id)
    .bind(ended)
    .bind(policy_snapshot(ended, 1_000, 100))
    .execute(pool)
    .await
    .expect("写入已结算场次");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'collection-wait-settlement', $2, $3, 'accepted')",
    )
    .bind(auction.command_id)
    .bind(auction.id)
    .bind([5_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入结算命令");
    auction
}

async fn seed_lot(
    pool: &PgPool,
    auction: &Auction,
    lot_number: i32,
    sold: bool,
    external_equipment_id: &str,
) -> Lot {
    let lot = Lot {
        id: stable_id(SEED, &format!("lot-{lot_number}")),
        equipment_id: stable_id(SEED, &format!("equipment-{lot_number}")),
    };
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'main', $2, $3, 'weapon', '{}'::jsonb, now() - interval '20 days')",
    )
    .bind(lot.equipment_id)
    .bind(external_equipment_id)
    .bind(format!("Collection wait equipment {lot_number}"))
    .execute(pool)
    .await
    .expect("写入测试装备");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, $4, $5, 1000, $6, now() - interval '10 days', 'weapon', $4)",
    )
    .bind(lot.id)
    .bind(auction.id)
    .bind(lot.equipment_id)
    .bind(lot_number)
    .bind(if sold { "sold" } else { "unsold" })
    .bind(sold.then_some(1_000_i64))
    .execute(pool)
    .await
    .expect("写入测试 lot");
    lot
}

/// 写入一条义务；买方义务以中标用户为收件人，卖方义务以冻结游戏名为收件人。
async fn seed_obligation(
    pool: &PgPool,
    auction: &Auction,
    lot: &Lot,
    kind: &str,
    amount: i64,
    prerequisite: Option<Uuid>,
) -> Uuid {
    let id = stable_id(SEED, &format!("{}-{kind}", lot.id));
    let buyer = matches!(kind, "winner_equipment_cod" | "buyer_refund");
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_user_id, recipient_game_identity,
             amount, carrier_credit, equipment_id, prerequisite_obligation_id, created_at,
             updated_at)
         VALUES ($1, $2, $3, 'main', $4, 1, $5, 'ready', $6, $7, $8, 0, $9, $10,
                 now() - interval '10 days', now() - interval '10 days')",
    )
    .bind(id)
    .bind(auction.id)
    .bind(lot.id)
    .bind(auction.command_id)
    .bind(kind)
    .bind(buyer.then_some(auction.winner))
    .bind((!buyer).then_some(SELLER))
    .bind(amount)
    .bind(lot.equipment_id)
    .bind(prerequisite)
    .execute(pool)
    .await
    .expect("写入测试义务");
    id
}

/// 写入义务的投递；`sent_at` 为 Some 时是已寄出（`succeeded`），否则仍在投递队列（`pending`）。
async fn seed_delivery(
    pool: &PgPool,
    obligation_id: Uuid,
    credits: i64,
    sent_at: Option<DateTime<Utc>>,
    external_mail_id: u32,
) -> Uuid {
    let id = stable_id(SEED, &format!("delivery-{obligation_id}"));
    let created_at = sent_at.unwrap_or_else(Utc::now) - Duration::minutes(5);
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": "settlement_collection_wait",
        "credits": credits,
        "cod": 0,
        "equipment_secret_refs": []
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata)
         VALUES ($1, $2, decode(repeat('77', 32), 'hex'), 'settlement', $3,
                 'collection-recipient', $4, $5, 1, $6, $7, $7, $7, $8, $9,
                 COALESCE($7, $9), '{\"operation\":\"send\"}'::jsonb)",
    )
    .bind(id)
    .bind(stable_id(SEED, &format!("delivery-command-{obligation_id}")))
    .bind(obligation_id.to_string())
    .bind(payload)
    .bind(if sent_at.is_some() { "succeeded" } else { "pending" })
    .bind(i32::from(sent_at.is_some()))
    .bind(sent_at)
    .bind(sent_at.map(|_| format!("mooglemail:main:send:{external_mail_id}")))
    .bind(created_at)
    .execute(pool)
    .await
    .expect("写入测试投递");
    id
}

/// 写入履约执行行；终态写完成时间，未完成阶段保持 NULL（表约束要求二者一致）。
async fn seed_fulfillment(
    pool: &PgPool,
    obligation_id: Uuid,
    execution_state: &str,
    delivery_id: Option<Uuid>,
    last_error_code: Option<&str>,
) {
    let terminal = matches!(execution_state, "satisfied" | "failed" | "manual_reconcile");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
             delivery_id, last_error_code, created_at, updated_at, completed_at)
         VALUES ($1, $2, 3, 'main', 'send', $3, $4, now() - interval '9 days',
                 now() - interval '9 days', $5)",
    )
    .bind(obligation_id)
    .bind(execution_state)
    .bind(delivery_id)
    .bind(last_error_code)
    .bind(terminal.then(|| Utc::now() - Duration::days(9)))
    .execute(pool)
    .await
    .expect("写入测试履约");
}

/// 已结算场次里的各类履约：
/// - lot 1：货款已寄出 8 天、资金已支出，最近一次 Sent 读取失败；
/// - lot 2：流拍退回已寄出 2 天，已读到但未领取；
/// - lot 3：货款尚在投递队列，未寄出；
/// - lot 4：COD 已寄出待付款，但没有冻结发送证明，不进入等待付款，仍按既有口径计积压；
/// - lot 5：流拍退回已寄出但证据不符，停在人工核对。
async fn seed_collection_wait(pool: &PgPool, now: DateTime<Utc>) -> Auction {
    let auction = seed_auction(pool, now).await;
    let paid = seed_lot(pool, &auction, 1, true, "9400001").await;
    let cod = seed_obligation(pool, &auction, &paid, "winner_equipment_cod", 1_000, None).await;
    seed_fulfillment(pool, cod, "satisfied", None, None).await;
    let payout = seed_obligation(pool, &auction, &paid, "seller_payout", 900, Some(cod)).await;
    let delivery = seed_delivery(pool, payout, 900, Some(now - Duration::days(8)), 700_001).await;
    seed_fulfillment(pool, payout, "enqueued", Some(delivery), Some("evidence_unavailable")).await;

    let unsold = seed_lot(pool, &auction, 2, false, "9400002").await;
    let unsold_return = seed_obligation(pool, &auction, &unsold, "unsold_return", 0, None).await;
    let delivery =
        seed_delivery(pool, unsold_return, 0, Some(now - Duration::days(2)), 700_002).await;
    seed_fulfillment(
        pool,
        unsold_return,
        "enqueued",
        Some(delivery),
        Some("evidence_not_observed"),
    )
    .await;

    let queued = seed_lot(pool, &auction, 3, true, "9400003").await;
    let cod = seed_obligation(pool, &auction, &queued, "winner_equipment_cod", 1_000, None).await;
    seed_fulfillment(pool, cod, "satisfied", None, None).await;
    let payout = seed_obligation(pool, &auction, &queued, "seller_payout", 500, Some(cod)).await;
    let delivery = seed_delivery(pool, payout, 500, None, 0).await;
    seed_fulfillment(pool, payout, "enqueued", Some(delivery), Some("evidence_not_observed")).await;

    let awaiting_payment = seed_lot(pool, &auction, 4, true, "9400004").await;
    let cod =
        seed_obligation(pool, &auction, &awaiting_payment, "winner_equipment_cod", 1_000, None)
            .await;
    let delivery = seed_delivery(pool, cod, 0, Some(now - Duration::days(1)), 700_004).await;
    seed_fulfillment(pool, cod, "enqueued", Some(delivery), Some("evidence_not_observed")).await;

    let disputed = seed_lot(pool, &auction, 5, false, "9400005").await;
    let disputed_return =
        seed_obligation(pool, &auction, &disputed, "unsold_return", 0, None).await;
    let delivery =
        seed_delivery(pool, disputed_return, 0, Some(now - Duration::days(3)), 700_005).await;
    seed_fulfillment(
        pool,
        disputed_return,
        "manual_reconcile",
        Some(delivery),
        Some("evidence_mismatch"),
    )
    .await;
    auction
}

fn policy() -> OperationsAlertPolicy {
    OperationsAlertPolicy {
        source_timeout: StdDuration::from_secs(5),
        batch_window: StdDuration::from_secs(120),
        cooldown: StdDuration::from_secs(1_800),
        claim_lease: StdDuration::from_secs(120),
        retry_delay: StdDuration::from_secs(300),
        max_signals_per_scan: 64,
        max_batch_items: 20,
        max_body_bytes: 4_096,
        delivery_pending_count_threshold: 1,
        delivery_pending_age_threshold: StdDuration::from_secs(900),
        delivery_manual_reconcile_count_threshold: 1,
        fulfillment_backlog_count_threshold: 1,
        fulfillment_backlog_age_threshold: StdDuration::from_secs(900),
        recovery_overdue_count_threshold: 1,
        recovery_overdue_age_threshold: StdDuration::from_secs(900),
    }
}

/// 等待领取不进积压：只有尚未寄出的货款与没有冻结发送证明的 COD 仍是待处理；
/// 等待领取单列计数、最近一次读取失败数与按寄出时刻计的最老等待年龄。
#[tokio::test]
async fn postgres_collection_wait_snapshot_separates_awaiting_from_backlog() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let now = Utc::now();
    seed_collection_wait(store.postgres_pool(), now).await;

    let snapshot = store.snapshot(now).await.expect("读取告警快照");
    let fulfillment = snapshot.fulfillment.expect("履约水位");
    assert_eq!(fulfillment.pending, 2, "未寄出货款与 COD 仍是待处理");
    assert_eq!(fulfillment.awaiting_collection, 2, "已寄出的货款与流拍退回进入等待领取");
    assert_eq!(fulfillment.awaiting_collection_unreadable, 1, "只有货款最近一次读取失败");
    assert_eq!(fulfillment.manual_reconcile, 1);
    assert_eq!(fulfillment.waiting_payment, 0);
    let eight_days = Duration::days(8).num_seconds();
    assert!(
        (eight_days..eight_days + 120)
            .contains(&fulfillment.oldest_awaiting_collection_age_seconds),
        "等待年龄按投递完成时刻计：{}",
        fulfillment.oldest_awaiting_collection_age_seconds
    );

    // 告警积压只计两条待处理；等待领取不再让积压计数膨胀。
    let observations = evaluate_snapshot(&snapshot, &policy()).expect("扫描评估");
    let backlog = observations
        .iter()
        .find(|observation| observation.source == OperationsAlertSource::Fulfillment)
        .expect("履约来源")
        .signals
        .iter()
        .find(|signal| signal.kind == OperationsAlertKind::FulfillmentBacklog)
        .expect("仍有未寄出的积压");
    assert_eq!(backlog.count, 2);

    store.postgres_pool().close().await;
}
