//! “已寄出、等待领取”在真实 PostgreSQL 18 schema 上的读侧口径，全部以运行账号执行：
//! - 告警/看板履约水位把已寄出的 Send 义务从积压拆到等待领取，单列最近一次读取失败；
//!   COD 仍按既有口径留在原分类，人工核对与尚未寄出的义务不变。
//! - 资产对账按持久寄出事实划分在途：已寄出且资金已 `spent` 的货款只有寄出不晚于余额观察才
//!   列为在途、不计入应付（观察早于寄出时仍计入应付，恰好相等按已扣除）；已寄出的装备（含已
//!   寄出待付款的 COD 装备）列为在途、不计为缺失；寄出后转人工核对的装备与现金单列为在途
//!   待核对，不重新计入应持或应付；资金分配缺失的已寄出货款仍按应付计。
//! - 托管投递分支按 canonical 附件 kind 识别真实结算载荷；仍在投递队列里的装备按应持核对，
//!   不因同一装备的另一义务已寄出而算作在途。
//! - 履约领取把投递完成时刻带进 delivery 快照，供等待领取的 Sent 轮询按寄出时长分档。
//! - 长期未领取按达到阈值的件数计数，Admin 清单与计数出自同一只读快照；告警在有超期件时
//!   写入活动信号，收口回落到阈值以下后由下一轮扫描关闭；迁移 000110 的闭集接受该种类。
//!
//! 入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration as StdDuration;

use auction_application::operations::{
    OperationsAlertKind, OperationsAlertObserveCommand, OperationsAlertPolicy,
    OperationsAlertRuntimeStatus, OperationsAlertSource, OperationsAlertSourceReader,
    OperationsAlertStore, evaluate_snapshot,
};
use auction_application::reconciliation::{
    AssetArmoryObservation, AssetReconciliationStore, CustodyInTransitReason, CustodyMissingSample,
    SolvencyInTransitReason,
};
use auction_application::settlement::{
    SettlementFulfillmentDeliveryStatus, SettlementFulfillmentState, SettlementFulfillmentStore,
};
use auction_domain::{SettlementObligationKind, SettlementWorld};
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

/// 写入一封仍在投递队列里、携带装备的结算 Send；载荷与结算生成方的 canonical 形状逐字段一致
/// （附件 kind 为 `auction_equipment_key`）。
async fn seed_queued_equipment_delivery(pool: &PgPool, obligation_id: Uuid, lot: &Lot) -> Uuid {
    let id = stable_id(SEED, &format!("queued-delivery-{obligation_id}"));
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": "settlement_equipment_handoff",
        "variables": {"item_name": "Collection wait equipment", "reference": obligation_id},
        "credits": 0,
        "cod": 0,
        "equipment_secret_refs": [{"kind": "auction_equipment_key", "id": lot.equipment_id}]
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('79', 32), 'hex'), 'settlement', $3,
                 'collection-recipient', $4, 'pending', $5, decode(repeat('7a', 32), 'hex'),
                 now())",
    )
    .bind(id)
    .bind(stable_id(SEED, &format!("queued-prepare-{obligation_id}")))
    .bind(obligation_id.to_string())
    .bind(payload)
    .bind(stable_id(SEED, &format!("queued-activate-{obligation_id}")))
    .execute(pool)
    .await
    .expect("写入队列中的装备投递");
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
        fulfillment_collection_overdue_age_threshold: StdDuration::from_secs(259_200),
        recovery_overdue_count_threshold: 1,
        recovery_overdue_age_threshold: StdDuration::from_secs(900),
    }
}

/// 等待领取不进积压：只有尚未寄出的货款与没有冻结发送证明的 COD 仍是待处理；
/// 等待领取单列计数、最近一次读取失败数与按寄出时刻计的最老等待年龄。长期未领取只计寄出
/// 时长达到阈值的件数，清单与计数出自同一只读快照：寄出 8 天的货款超期，寄出 2 天的退回不超期；
/// 不给阈值时不评估长期未领取。
#[tokio::test]
async fn postgres_collection_wait_snapshot_separates_awaiting_from_backlog() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let now = Utc::now();
    seed_collection_wait(store.postgres_pool(), now).await;

    let threshold = policy().fulfillment_collection_overdue_age_threshold;
    let snapshot = store.snapshot(now, Some(threshold)).await.expect("读取告警快照");
    let fulfillment = snapshot.fulfillment.expect("履约水位");
    assert_eq!(fulfillment.collection_overdue, 1, "只有寄出 8 天的货款达到 72 小时阈值");
    let payout = stable_id(SEED, &format!("{}-seller_payout", stable_id(SEED, "lot-1")));
    let [item] = snapshot.collection_overdue_items.as_slice() else {
        panic!("长期未领取清单只应有一项：{:?}", snapshot.collection_overdue_items);
    };
    assert_eq!(item.obligation_id, payout);
    assert_eq!(item.obligation_kind, SettlementObligationKind::SellerPayout);
    assert_eq!((item.auction_world.as_str(), item.auction_sequence), ("main", 4));
    assert_eq!((item.lot_number, item.equipment_category.as_str()), (1, "weapon"));
    assert_eq!(item.equipment_name, "Collection wait equipment 1");
    assert_eq!(item.recipient, "collection-recipient");
    assert!(item.last_read_failed, "货款最近一次读取失败");
    assert_eq!(item.age_seconds, fulfillment.oldest_awaiting_collection_age_seconds);
    assert!(item.sent_at <= now - Duration::days(8));
    let unevaluated = store.snapshot(now, None).await.expect("读取告警快照");
    assert_eq!(unevaluated.fulfillment.map(|value| value.collection_overdue), Some(0));
    assert!(unevaluated.collection_overdue_items.is_empty());
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
/// 寄出后转人工核对的流拍退回按寄出事实计在途待核对，不再计入应持或缺失。报告追加后经最近
/// 报告投影与告警快照原样读回。
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
    assert_eq!((main.expected_count, main.missing_count), (0, 0), "已寄出的装备都不再应持");
    assert!(main.missing_samples.is_empty());
    assert!(custody.consistent, "在途与在途待核对都不是缺失");
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
    // 寄出后证据不符转人工核对的流拍退回：按寄出事实计在途待核对，不再算“应在 Armory”。
    assert_eq!(main.in_transit_review_count, 1);
    assert_eq!(
        main.in_transit_review_reasons,
        [CustodyInTransitReason {
            reason: "obligation_unsold_return_manual_reconcile".to_owned(),
            count: 1,
        }]
    );
    assert!(!custody.asset_actions_authorized && !solvency.asset_actions_authorized);

    let latest = store.latest_asset_reports().await.expect("读取最近报告");
    assert_eq!(latest.custody.as_ref(), Some(&custody));
    assert_eq!(latest.solvency.as_ref(), Some(&solvency));
    let snapshot = store.snapshot(Utc::now(), None).await.expect("读取告警快照");
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
        (0, 2, 0, 1, 900)
    );

    pool.close().await;
}

/// 写入 main 世界的余额观察；`observed_at` 为 None 时取投递 `delivery_id` 的完成时刻（与寄出
/// 时刻逐值相等）。
async fn observe_balance(
    pool: &PgPool,
    credits: i64,
    observed_at: Option<DateTime<Utc>>,
    delivery_id: Uuid,
) {
    sqlx::query(
        "UPDATE public.settlement_recovery_health
         SET balance_credits = $1,
             balance_observed_at = COALESCE($2,
                 (SELECT completed_at FROM public.deliveries WHERE id = $3)),
             balance_status = 'ok', evidence_status = 'fresh', updated_at = now(),
             evidence_last_success_at = COALESCE($2,
                 (SELECT completed_at FROM public.deliveries WHERE id = $3))
         WHERE world_id = 'main'",
    )
    .bind(credits)
    .bind(observed_at)
    .bind(delivery_id)
    .execute(pool)
    .await
    .expect("写入余额观察");
}

/// 一笔已支出的卖方货款：结算 COD 已满足，货款按 `sent_at` 寄出（None 时仍在队列）。
async fn seed_payout(
    pool: &PgPool,
    auction: &Auction,
    lot_number: i32,
    amount: i64,
    sent_at: Option<DateTime<Utc>>,
    execution_state: &str,
    spent: bool,
) -> Uuid {
    let lot = seed_lot(pool, auction, lot_number, true, &format!("94001{lot_number:02}")).await;
    let cod = seed_obligation(pool, auction, &lot, "winner_equipment_cod", 1_000, None).await;
    seed_fulfillment(pool, cod, "satisfied", None, None).await;
    let payout = seed_obligation(pool, auction, &lot, "seller_payout", amount, Some(cod)).await;
    let mail_id = 710_000 + u32::try_from(lot_number).expect("lot 编号");
    let delivery = seed_delivery(pool, payout, amount, sent_at, mail_id).await;
    let error_code = if execution_state == "manual_reconcile" {
        "evidence_mismatch"
    } else {
        "evidence_not_observed"
    };
    seed_fulfillment(pool, payout, execution_state, Some(delivery), Some(error_code)).await;
    if spent {
        seed_spent_allocation(pool, &lot, payout, delivery, amount).await;
    }
    delivery
}

/// 偿付的在途排除与余额观察时点对齐（处理决定第 1 条的例子）：观察到余额 1000 之后才寄出 900、
/// 另有待付 500 时，观察到的余额仍含这 900，必须判为不足 400；寄出后的新观察（余额 100）与恰好
/// 等于寄出时刻的观察都已扣除这笔钱，900 计在途，同样不足 400。错把寄出晚于观察的支出算作在途
/// 会误判充足，错把观察时刻已扣除的支出算作应付会误报 1300 的缺口。
#[tokio::test]
async fn postgres_collection_wait_solvency_aligns_in_transit_with_balance_observation() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let now = Utc::now();
    let auction = seed_auction(pool, now).await;
    let sent =
        seed_payout(pool, &auction, 1, 900, Some(now - Duration::hours(1)), "enqueued", true).await;
    seed_payout(pool, &auction, 2, 500, None, "enqueued", false).await;
    let report = || {
        let store = &store;
        async move {
            let solvency = store.report_solvency(86_400).await.expect("生成偿付报告");
            let main =
                solvency.worlds.into_iter().find(|world| world.world == "main").expect("main 报告");
            (
                main.pending_seller_payout_credits,
                main.in_transit_seller_payout_credits,
                main.required_credits,
                main.verdict,
                main.shortfall_credits,
            )
        }
    };

    observe_balance(pool, 1_000, Some(now - Duration::hours(2)), sent).await;
    assert_eq!(
        report().await,
        (1_400, 0, 1_400, "shortfall".to_owned(), 400),
        "余额观察早于寄出：观察到的余额仍含这笔货款"
    );

    observe_balance(pool, 100, Some(Utc::now()), sent).await;
    assert_eq!(
        report().await,
        (500, 900, 500, "shortfall".to_owned(), 400),
        "余额观察晚于寄出：货款已离开余额，计在途"
    );

    observe_balance(pool, 100, None, sent).await;
    assert_eq!(
        report().await,
        (500, 900, 500, "shortfall".to_owned(), 400),
        "观察时刻恰好等于寄出时刻：按已扣除计在途"
    );

    pool.close().await;
}

/// 已寄出、已支出的货款在寄出后转人工核对：按寄出事实计在途待核对（笔数、金额与原因），不重新
/// 计入应付；资金分配缺失的已寄出货款不能证明已支出，仍按应付计。
#[tokio::test]
async fn postgres_collection_wait_sent_cash_in_manual_review_stays_in_transit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let now = Utc::now();
    let auction = seed_auction(pool, now).await;
    let reviewed = seed_payout(
        pool,
        &auction,
        1,
        700,
        Some(now - Duration::hours(3)),
        "manual_reconcile",
        true,
    )
    .await;
    seed_payout(pool, &auction, 2, 300, Some(now - Duration::hours(3)), "enqueued", false).await;
    observe_balance(pool, 1_000, Some(now), reviewed).await;

    let solvency = store.report_solvency(86_400).await.expect("生成偿付报告");
    let main = solvency.worlds.iter().find(|world| world.world == "main").expect("main 报告");
    assert_eq!(
        (main.pending_seller_payout_count, main.pending_seller_payout_credits),
        (1, 300),
        "没有资金分配的已寄出货款仍按应付计"
    );
    assert_eq!(
        (main.in_transit_seller_payout_count, main.in_transit_seller_payout_credits),
        (0, 0)
    );
    assert_eq!((main.in_transit_review_count, main.in_transit_review_credits), (1, 700));
    assert_eq!(
        main.in_transit_review_reasons,
        [SolvencyInTransitReason {
            reason: "obligation_seller_payout_manual_reconcile".to_owned(),
            count: 1,
            credits: 700,
        }]
    );
    assert_eq!(main.required_credits, 300, "转人工核对的已寄出货款不重新计入应付");
    assert_eq!((main.verdict.as_str(), main.shortfall_credits), ("sufficient", 0));
    assert!(!solvency.asset_actions_authorized);

    pool.close().await;
}

/// 托管投递分支识别真实 canonical 结算载荷：同一件装备的流拍退回已寄出（本可算在途），但另有
/// 一封携带它的结算投递仍在队列里，装备必须按应持核对（原因 `delivery_pending_equipment`），
/// 不得算作在途。
#[tokio::test]
async fn postgres_collection_wait_queued_canonical_delivery_keeps_equipment_expected() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let now = Utc::now();
    let auction = seed_auction(pool, now).await;
    let lot = seed_lot(pool, &auction, 1, false, "9400031").await;
    let unsold_return = seed_obligation(pool, &auction, &lot, "unsold_return", 0, None).await;
    let delivery =
        seed_delivery(pool, unsold_return, 0, Some(now - Duration::days(2)), 700_031).await;
    seed_fulfillment(
        pool,
        unsold_return,
        "enqueued",
        Some(delivery),
        Some("evidence_not_observed"),
    )
    .await;
    seed_queued_equipment_delivery(pool, unsold_return, &lot).await;

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
    assert_eq!(custody.failure_code, None);
    let main = custody.worlds.iter().find(|world| world.world == "main").expect("main 报告");
    assert_eq!((main.expected_count, main.missing_count), (1, 1));
    assert_eq!(
        main.missing_samples,
        [CustodyMissingSample {
            external_equipment_id: "9400031".to_owned(),
            reason: "delivery_pending_equipment".to_owned(),
        }]
    );
    assert_eq!(main.in_transit_count, 0, "仍有应持来源的装备不算在途");
    assert!(main.in_transit_reasons.is_empty());

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

/// 长期未领取：最久一笔已寄出 8 天（阈值 72 小时）时产生活动信号，计数是达到阈值的件数
/// （另一笔寄出 2 天的退回只是背景，不计入）；该笔领取收口后剩下的只寄出 2 天，下一轮扫描
/// 关闭信号。告警不改变任何义务状态。
#[tokio::test]
async fn postgres_collection_wait_overdue_alert_triggers_and_resolves() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let now = Utc::now();
    seed_collection_wait(pool, now).await;
    store
        .publish_runtime_status(OperationsAlertRuntimeStatus { enabled: true, updated_at: now })
        .await
        .expect("启用告警扫描");
    let scan = |at: DateTime<Utc>| {
        let store = &store;
        async move {
            let threshold = policy().fulfillment_collection_overdue_age_threshold;
            let snapshot = store.snapshot(at, Some(threshold)).await.expect("读取告警快照");
            let observations = evaluate_snapshot(&snapshot, &policy()).expect("扫描评估");
            store
                .observe(OperationsAlertObserveCommand {
                    observations,
                    now: at,
                    batch_window: StdDuration::from_secs(120),
                })
                .await
                .expect("写入扫描观察");
        }
    };
    let overdue = || {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (bool, i64, i64)>(
                "SELECT active, count_value, age_seconds FROM public.operations_alert_signals
                 WHERE kind = 'fulfillment_collection_overdue'
                   AND source = 'fulfillment' AND scope = 'auction'",
            )
            .fetch_optional(&pool)
            .await
            .expect("读取长期未领取信号")
        }
    };

    scan(now).await;
    let (active, count, age) = overdue().await.expect("超过阈值必须写入信号");
    assert!(active);
    assert_eq!(count, 1, "计数是达到阈值的件数，不是全部等待领取");
    assert!(age >= Duration::days(8).num_seconds(), "年龄是最久寄出时长：{age}");

    // 8 天前寄出的货款被领取：只剩寄出 2 天的流拍退回，低于阈值，信号关闭。
    let payout = stable_id(SEED, &format!("{}-seller_payout", stable_id(SEED, "lot-1")));
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'satisfied', completed_at = now(), updated_at = now(),
             last_error_code = NULL
         WHERE obligation_id = $1",
    )
    .bind(payout)
    .execute(pool)
    .await
    .expect("模拟货款已领取");
    scan(Utc::now()).await;
    let (active, _, _) = overdue().await.expect("信号行保留历史");
    assert!(!active, "回落到阈值以下后告警解除");

    pool.close().await;
}
