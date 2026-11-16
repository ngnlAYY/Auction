//! “已寄出、等待领取”在真实 PostgreSQL 18 schema 上的读侧口径，全部以运行账号执行：
//! - 告警/看板履约水位把已寄出的 Send 义务从积压拆到等待领取，单列最近一次读取失败；
//!   COD 仍按既有口径留在原分类，人工核对与尚未寄出的义务不变。
//! - 资产对账把已寄出且资金已 `spent` 的货款列为在途、不计入应付；已寄出等待领取的装备
//!   （含已寄出待付款的 COD 装备）列为在途、不计为缺失；人工核对的仍按应持核对。
//! - 履约领取把投递完成时刻带进 delivery 快照，供等待领取的 Sent 轮询按寄出时长分档。
//!
//! 入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration as StdDuration;

use auction_application::operations::{
    OperationsAlertKind, OperationsAlertPolicy, OperationsAlertSource, OperationsAlertSourceReader,
    evaluate_snapshot,
};
use auction_application::reconciliation::{
    AssetArmoryObservation, AssetReconciliationStore, CustodyInTransitReason, CustodyMissingSample,
};
use auction_application::settlement::{
    SettlementFulfillmentDeliveryStatus, SettlementFulfillmentState, SettlementFulfillmentStore,
};
use auction_domain::SettlementWorld;
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
             external_reference, created_at, updated_at, result_metadata,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('77', 32), 'hex'), 'settlement', $3,
                 'collection-recipient', $4, $5, 1, $6, $7, $7, $7, $8, $9,
                 COALESCE($7, $9), '{\"operation\":\"send\"}'::jsonb,
                 $10, decode(repeat('78', 32), 'hex'), $9)",
    )
    .bind(id)
    .bind(stable_id(SEED, &format!("prepare-{obligation_id}")))
    .bind(obligation_id.to_string())
    .bind(payload)
    .bind(if sent_at.is_some() { "succeeded" } else { "pending" })
    .bind(i32::from(sent_at.is_some()))
    .bind(sent_at)
    .bind(sent_at.map(|_| format!("mooglemail:main:send:{external_mail_id}")))
    .bind(created_at)
    .bind(stable_id(SEED, &format!("activate-{obligation_id}")))
    .execute(pool)
    .await
    .expect("写入测试投递");
    id
}

/// 写入履约执行行；终态写完成时间，未完成阶段保持 NULL（表约束要求二者一致）。有投递时
/// 同时写入与投递一致的 prepare/activate 身份，使行形状满足领取前的执行身份校验。
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
             delivery_id, last_error_code, created_at, updated_at, completed_at,
             prepare_command_id, prepare_request_digest, activate_command_id,
             activate_request_digest)
         VALUES ($1, $2, 3, 'main', 'send', $3, $4, now() - interval '9 days',
                 now() - interval '9 days', $5, $6,
                 CASE WHEN $3::uuid IS NULL THEN NULL ELSE decode(repeat('77', 32), 'hex') END,
                 $7,
                 CASE WHEN $3::uuid IS NULL THEN NULL ELSE decode(repeat('78', 32), 'hex') END)",
    )
    .bind(obligation_id)
    .bind(execution_state)
    .bind(delivery_id)
    .bind(last_error_code)
    .bind(terminal.then(|| Utc::now() - Duration::days(9)))
    .bind(delivery_id.map(|_| stable_id(SEED, &format!("prepare-{obligation_id}"))))
    .bind(delivery_id.map(|_| stable_id(SEED, &format!("activate-{obligation_id}"))))
    .execute(pool)
    .await
    .expect("写入测试履约");
}

/// 货款寄出时资金分配记为 `spent`（与发送事务同一 owner 写入），这里直接写入终态事实。
async fn seed_spent_allocation(
    pool: &PgPool,
    lot: &Lot,
    obligation_id: Uuid,
    delivery_id: Uuid,
    amount: i64,
) {
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts (uid, status) VALUES ('7001', 'ready')
         ON CONFLICT (uid) DO NOTHING",
    )
    .execute(pool)
    .await
    .expect("写入资金账户");
    sqlx::query(
        "INSERT INTO public.settlement_funding_allocations
            (id, uid, purpose, lot_id, obligation_id, amount, state, delivery_id)
         VALUES ($1, '7001', 'seller', $2, $3, $4, 'spent', $5)",
    )
    .bind(stable_id(SEED, &format!("allocation-{obligation_id}")))
    .bind(lot.id)
    .bind(obligation_id)
    .bind(amount)
    .bind(delivery_id)
    .execute(pool)
    .await
    .expect("写入已支出的资金分配");
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
    seed_spent_allocation(pool, &paid, payout, delivery, 900).await;

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

/// 已寄出且资金已支出的货款进在途、不进待付与缺口；已寄出等待领取的装备进在途、不进缺失；
/// 人工核对的流拍退回仍按应持核对。报告追加后经最近报告投影与告警快照原样读回。
#[tokio::test]
async fn postgres_collection_wait_reports_sent_assets_in_transit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let now = Utc::now();
    seed_collection_wait(pool, now).await;
    // 余额 600 足以支付尚未寄出的 500；若把已寄出的 900 再算一次，会误报 800 的缺口。
    sqlx::query(
        "UPDATE public.settlement_recovery_health
         SET balance_credits = 600, balance_observed_at = now(), balance_status = 'ok',
             evidence_last_success_at = now(), evidence_status = 'fresh', updated_at = now()
         WHERE world_id = 'main'",
    )
    .execute(pool)
    .await
    .expect("写入新鲜余额");

    let solvency = store.report_solvency(3_600).await.expect("生成偿付报告");
    assert_eq!(solvency.format, "auction-solvency-report.v2");
    let main = solvency.worlds.iter().find(|world| world.world == "main").expect("main 报告");
    assert_eq!(
        (main.pending_seller_payout_count, main.pending_seller_payout_credits),
        (1, 500),
        "只有尚未寄出的货款仍待付"
    );
    assert_eq!(
        (main.in_transit_seller_payout_count, main.in_transit_seller_payout_credits),
        (1, 900),
        "已寄出且已支出的货款单列为在途"
    );
    assert_eq!((main.in_transit_refund_count, main.in_transit_refund_credits), (0, 0));
    assert_eq!(main.required_credits, 500);
    assert_eq!((main.verdict.as_str(), main.shortfall_credits), ("sufficient", 0));

    let observation = |world| AssetArmoryObservation {
        world,
        observed_at: Utc::now(),
        equipment_ids: Vec::new(),
        complete: true,
        failure_code: None,
    };
    let custody = store
        .reconcile_custody(vec![
            observation(SettlementWorld::Isekai),
            observation(SettlementWorld::Main),
        ])
        .await
        .expect("生成装备持有报告");
    assert_eq!(custody.format, "auction-custody-reconciliation.v2");
    assert_eq!(custody.failure_code, None);
    let main = custody.worlds.iter().find(|world| world.world == "main").expect("main 报告");
    assert_eq!((main.expected_count, main.missing_count), (1, 1), "只有人工核对的退回仍应持");
    assert_eq!(
        main.missing_samples,
        [CustodyMissingSample {
            external_equipment_id: "9400005".to_owned(),
            reason: "obligation_unsold_return".to_owned(),
        }]
    );
    assert_eq!(main.in_transit_count, 2);
    assert_eq!(
        main.in_transit_reasons,
        [
            CustodyInTransitReason { reason: "obligation_unsold_return".to_owned(), count: 1 },
            CustodyInTransitReason {
                reason: "obligation_winner_equipment_cod".to_owned(),
                count: 1,
            },
        ]
    );
    assert!(!custody.asset_actions_authorized && !solvency.asset_actions_authorized);

    let latest = store.latest_asset_reports().await.expect("读取最近报告");
    assert_eq!(latest.custody.as_ref(), Some(&custody));
    assert_eq!(latest.solvency.as_ref(), Some(&solvency));
    let snapshot = store.snapshot(Utc::now()).await.expect("读取告警快照");
    let reconciliation = snapshot.reconciliation.expect("对账水位");
    let main = reconciliation.worlds.iter().find(|world| world.world == "main").expect("main");
    assert_eq!(
        (
            main.custody_missing_count,
            main.custody_in_transit_count,
            main.solvency_shortfall_credits,
            main.solvency_in_transit_count,
            main.solvency_in_transit_credits,
        ),
        (1, 2, 0, 1, 900)
    );

    pool.close().await;
}

/// 领取已寄出的货款时，delivery 快照带出数据库投递完成时刻（寄出时刻）；应用层据此分档，
/// 不另取时钟来源。
#[tokio::test]
async fn postgres_collection_wait_claim_carries_sent_time() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let now = Utc::now();
    // 数据库时间只到微秒；用微秒精度的寄出时刻才能逐值比较。
    let sent_at = DateTime::from_timestamp_micros((now - Duration::days(8)).timestamp_micros())
        .expect("寄出时刻");
    let auction = seed_auction(pool, now).await;
    let lot = seed_lot(pool, &auction, 1, true, "9400011").await;
    let cod = seed_obligation(pool, &auction, &lot, "winner_equipment_cod", 1_000, None).await;
    seed_fulfillment(pool, cod, "satisfied", None, None).await;
    let payout = seed_obligation(pool, &auction, &lot, "seller_payout", 900, Some(cod)).await;
    let delivery = seed_delivery(pool, payout, 900, Some(sent_at), 700_011).await;
    seed_fulfillment(pool, payout, "enqueued", Some(delivery), Some("evidence_not_observed")).await;

    let claims = store
        .claim_due("collection-wait-worker".to_owned(), now, now + Duration::seconds(60), 10)
        .await
        .expect("领取到期履约");
    let claim = claims
        .iter()
        .find(|claim| claim.obligation_id.as_uuid() == payout)
        .expect("领取已寄出的货款");
    assert_eq!(claim.state, SettlementFulfillmentState::Enqueued);
    let snapshot = claim.delivery_snapshot.as_ref().expect("投递快照");
    assert_eq!(snapshot.status, SettlementFulfillmentDeliveryStatus::Succeeded);
    assert_eq!(snapshot.completed_at, Some(sent_at));

    pool.close().await;
}
