//! COD 与载体付款在“等待领取”拆分后的完整路径，全部以运行账号执行：
//! - 付款投递只经投递端口按 Worker 的真实顺序推进（领取 → `start_sending_authorized` 带冻结
//!   Send 身份 → `succeed` 绑定唯一 Sent MID），数据库据此判定冻结发送证明成立；
//! - 恢复记录在付款期内时计为等待买家付款（`waiting_payment = 1`），不进入等待领取、积压或
//!   长期未领取清单，即使寄出已远超长期未领取阈值；
//! - 买家付款领取后，履约经领取证明满足并解除依赖；存在受保护资金账户时，旧满足入口拒绝 COD，
//!   必须走资金化校验，义务保持等待付款；
//! - 反例：付款投递已成功但没有冻结发送证明时不计为等待付款。
//!
//! 入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendExpectation, DeliverySendResult, DeliveryWorkStore,
};
use auction_application::operations::{
    OperationsAlertCollectionOverdueRead, OperationsAlertFulfillmentSnapshot,
    OperationsAlertSnapshot, OperationsAlertSourceReader,
};
use auction_application::settlement::{
    SettlementFulfillmentProof, SettlementFulfillmentReconcileStatus, SettlementFulfillmentStore,
};
use auction_storage::DatabaseStore;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

const SEED: &str = "collection-cod";
const WINNER: &str = "collection-cod-winner";
const SELLER: &str = "collection-cod-seller";
const BUYER_CREDITS: i64 = 1_000;
const THRESHOLD: StdDuration = StdDuration::from_secs(259_200);

/// 付款义务的两种形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Payment {
    /// main 场次：COD 邮件附装备，收件人付款即取得装备。
    EquipmentCod,
    /// isekai 场次：在 main 寄 1 Credit 载体 COD，付款后再在异世界交付装备。
    CarrierCod,
}

impl Payment {
    fn world(self) -> &'static str {
        match self {
            Self::EquipmentCod => "main",
            Self::CarrierCod => "isekai",
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Self::EquipmentCod => "winner_equipment_cod",
            Self::CarrierCod => "winner_payment_carrier_cod",
        }
    }

    fn carrier_credit(self) -> i64 {
        i64::from(self == Self::CarrierCod)
    }
}

/// 一个付款场景的标识。
struct CodCase {
    payment: Uuid,
    payout: Uuid,
    delivery: Uuid,
    external_equipment_id: String,
}

/// 写入已结算场次、lot、付款义务及其依赖（卖方货款受阻于付款或交付、买方退款为或有）、付款
/// 投递（尚在队列）、各履约行与付款期内的恢复记录。`payment_deadline` 是恢复记录冻结的期限。
async fn seed_case(pool: &PgPool, payment: Payment, payment_deadline: DateTime<Utc>) -> CodCase {
    let world = payment.world();
    let id = |suffix: &str| stable_id(SEED, &format!("{world}-{suffix}"));
    let winner = id("winner");
    seed_user(pool, winner, WINNER).await;
    let ended = Utc::now() - Duration::days(1);
    let auction = id("auction");
    let command = id("command");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, $2, 7, 'collection cod', 'settled', $3, $3, $3, $3, $3, $3, $4)",
    )
    .bind(auction)
    .bind(world)
    .bind(ended)
    .bind(policy_snapshot(ended, BUYER_CREDITS, 100))
    .execute(pool)
    .await
    .expect("写入已结算场次");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'collection-cod-settlement', $2, $3, 'accepted')",
    )
    .bind(command)
    .bind(auction)
    .bind([6_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入结算命令");
    let equipment = id("equipment");
    let external_equipment_id =
        if payment == Payment::EquipmentCod { "9400401" } else { "9400402" }.to_owned();
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, $2, $3, 'Collection cod equipment', 'weapon', '{}'::jsonb, $4)",
    )
    .bind(equipment)
    .bind(world)
    .bind(&external_equipment_id)
    .bind(ended - Duration::days(2))
    .execute(pool)
    .await
    .expect("写入测试装备");
    let lot = id("lot");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'sold', $4, $4, $5, 'weapon', 1)",
    )
    .bind(lot)
    .bind(auction)
    .bind(equipment)
    .bind(BUYER_CREDITS)
    .bind(ended)
    .execute(pool)
    .await
    .expect("写入测试 lot");

    let obligation = |kind: &str, state: &str, buyer: bool, amount: i64, prerequisite| {
        let obligation_id = id(kind);
        let kind = kind.to_owned();
        let state = state.to_owned();
        async move {
            sqlx::query(
                "INSERT INTO public.settlement_obligations
                    (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                     obligation_kind, business_state, recipient_user_id, recipient_game_identity,
                     amount, carrier_credit, equipment_id, prerequisite_obligation_id)
                 VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(obligation_id)
            .bind(auction)
            .bind(lot)
            .bind(world)
            .bind(command)
            .bind(&kind)
            .bind(&state)
            .bind(buyer.then_some(winner))
            .bind((!buyer).then_some(SELLER))
            .bind(amount)
            .bind(if kind == "winner_payment_carrier_cod" { 1_i64 } else { 0 })
            .bind(equipment)
            .bind(prerequisite)
            .execute(pool)
            .await
            .expect("写入测试义务");
            obligation_id
        }
    };
    let payment_id = obligation(payment.kind(), "ready", true, BUYER_CREDITS, None).await;
    let handoff = match payment {
        Payment::EquipmentCod => None,
        Payment::CarrierCod => {
            Some(obligation("winner_equipment_handoff", "blocked", true, 0, Some(payment_id)).await)
        }
    };
    let payout =
        obligation("seller_payout", "blocked", false, 900, Some(handoff.unwrap_or(payment_id)))
            .await;
    let refund =
        obligation("buyer_refund", "contingent", true, BUYER_CREDITS, Some(payment_id)).await;

    // 付款投递：与结算生成方同形的冻结载荷，激活后排队等待投递 Worker 领取。
    let delivery = id("delivery");
    let carrier = payment.carrier_credit();
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": match payment {
            Payment::EquipmentCod => "settlement_winner_equipment_cod",
            Payment::CarrierCod => "settlement_payment_carrier",
        },
        "variables": {
            "reference": payment_id.to_string(),
            "amount": (BUYER_CREDITS + carrier).to_string(),
            "item_name": "Collection cod equipment"
        },
        "credits": carrier,
        "cod": BUYER_CREDITS + carrier,
        "equipment_secret_refs": match payment {
            Payment::EquipmentCod => json!([{"kind": "auction_equipment_key", "id": equipment}]),
            Payment::CarrierCod => json!([]),
        }
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('a1', 32), 'hex'), 'settlement', $3, $4, $5, 'pending',
                 $6, decode(repeat('a2', 32), 'hex'), now())",
    )
    .bind(delivery)
    .bind(id("prepare"))
    .bind(payment_id.to_string())
    .bind(WINNER)
    .bind(payload)
    .bind(id("activate"))
    .execute(pool)
    .await
    .expect("写入付款投递");

    let fulfillment = |obligation_id: Uuid, state: &str, dispatch: &str, delivery: Option<Uuid>| {
        let state = state.to_owned();
        let dispatch = dispatch.to_owned();
        async move {
            sqlx::query(
                "INSERT INTO public.settlement_obligation_fulfillments
                    (obligation_id, execution_state, dispatch_world, delivery_operation,
                     delivery_id, prepare_command_id, prepare_request_digest,
                     activate_command_id, activate_request_digest)
                 VALUES ($1, $2, $3, 'send', $4, $5,
                         CASE WHEN $4::uuid IS NULL THEN NULL
                              ELSE decode(repeat('a1', 32), 'hex') END,
                         $6,
                         CASE WHEN $4::uuid IS NULL THEN NULL
                              ELSE decode(repeat('a2', 32), 'hex') END)",
            )
            .bind(obligation_id)
            .bind(&state)
            .bind(&dispatch)
            .bind(delivery)
            .bind(delivery.map(|_| id("prepare")))
            .bind(delivery.map(|_| id("activate")))
            .execute(pool)
            .await
            .expect("写入测试履约");
        }
    };
    fulfillment(payment_id, "enqueued", "main", Some(delivery)).await;
    if let Some(handoff) = handoff {
        fulfillment(handoff, "pending", "isekai", None).await;
    }
    fulfillment(payout, "pending", "main", None).await;
    fulfillment(refund, "pending", "main", None).await;

    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, equipment_obligation_id, seller_payout_obligation_id,
             buyer_refund_obligation_id, policy_version, payment_deadline_at, stage)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 2, $11, 'awaiting_payment')",
    )
    .bind(id("recovery"))
    .bind(auction)
    .bind(lot)
    .bind(world)
    .bind(equipment)
    .bind(winner)
    .bind(payment_id)
    .bind(handoff)
    .bind(payout)
    .bind(refund)
    .bind(payment_deadline)
    .execute(pool)
    .await
    .expect("写入付款期内的恢复记录");
    CodCase { payment: payment_id, payout, delivery, external_equipment_id }
}

/// 按 Worker 的真实顺序经投递端口寄出付款邮件：领取 → 以冻结 Send 身份进入 sending →
/// 以绑定唯一 Sent MID 的成功证明收口。
async fn send_through_fence(store: &DatabaseStore, payment: Payment, case: &CodCase) {
    let now = Utc::now();
    let sending_grace = StdDuration::from_secs(60);
    DeliveryWorkStore::begin_claim_cycle(store, now, sending_grace).await.expect("领取前清理");
    let claims = DeliveryWorkStore::claim_due(
        store,
        "collection-cod-delivery".to_owned(),
        now,
        now + Duration::minutes(2),
        sending_grace,
        DeliveryClaimExclusions::default(),
        10,
    )
    .await
    .expect("领取投递");
    let claim =
        claims.into_iter().find(|claim| claim.delivery_id == case.delivery).expect("领取付款投递");
    let carrier = payment.carrier_credit();
    let expectation = Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: format!("auction payment request [{}]", case.payment),
        body_digest: [0x5a; 32],
        credits: carrier,
        cod: BUYER_CREDITS + carrier,
        equipment_ids: match payment {
            Payment::EquipmentCod => vec![case.external_equipment_id.clone()],
            Payment::CarrierCod => Vec::new(),
        },
        credential: None,
    });
    let prepared = DeliveryPreparedExpectation::Send(Arc::clone(&expectation));
    let outcome = store
        .start_sending_authorized(claim.clone(), Utc::now(), None, Some(&prepared))
        .await
        .expect("发送围栏必须确定完成");
    assert!(matches!(outcome, DeliveryFenceOutcome::Sending), "合法冻结身份必须进入 sending");
    let external_mail_id = if payment == Payment::EquipmentCod { 7_400_401 } else { 7_400_402 };
    store
        .succeed(
            claim.clone(),
            DeliverySendResult {
                external_reference: format!("mooglemail:main:send:{external_mail_id}"),
                result_metadata: json!({
                    "operation": "send",
                    "external_reference_proven": true,
                    "sent_verified": true,
                    "proof": "sent_exact_payload_v1",
                    "sending_fencing_version": claim.fencing_version,
                    "send_expectation": serde_json::to_value(&*expectation).expect("冻结身份"),
                }),
            },
            Utc::now(),
        )
        .await
        .expect("合法 Sent 证明必须收口");
}

async fn frozen_sent_proof(pool: &PgPool, delivery: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT public.delivery_has_frozen_sent_proof(id, result_metadata, external_reference)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("读取冻结发送证明判定")
}

/// 以 `at` 观察履约水位与长期未领取清单（带清单读取）。
async fn observe(store: &DatabaseStore, at: DateTime<Utc>) -> OperationsAlertSnapshot {
    store
        .snapshot(at, OperationsAlertCollectionOverdueRead::CountWithItems(THRESHOLD))
        .await
        .expect("读取告警快照")
}

/// 断言用的分组：等待付款、等待领取、待处理、受阻、等待前置、长期未领取超期件数。
fn grouping(fulfillment: &OperationsAlertFulfillmentSnapshot) -> [i64; 6] {
    [
        fulfillment.waiting_payment,
        fulfillment.awaiting_collection,
        fulfillment.pending,
        fulfillment.blocked,
        fulfillment.waiting_prerequisite,
        fulfillment.collection_overdue,
    ]
}

/// 领取付款义务的履约，按买家付款后的 Sent 领取证明走旧满足入口。
async fn satisfy_collected(
    store: &DatabaseStore,
    case: &CodCase,
) -> Result<SettlementFulfillmentReconcileStatus, auction_application::core::ApplicationError> {
    let now = Utc::now();
    let claims = SettlementFulfillmentStore::claim_due(
        store,
        "collection-cod-fulfillment".to_owned(),
        now,
        now + Duration::seconds(60),
        10,
    )
    .await
    .expect("领取到期履约");
    let claim = claims
        .into_iter()
        .find(|claim| claim.obligation_id.as_uuid() == case.payment)
        .expect("领取付款履约");
    let snapshot = claim.delivery_snapshot.as_ref().expect("投递快照");
    assert!(snapshot.external_reference_proven, "付款投递的 Sent 引用已证明");
    let reference = snapshot
        .external_reference
        .as_deref()
        .and_then(|reference| reference.rsplit(':').next())
        .expect("Sent MID")
        .to_owned();
    store
        .satisfy(
            claim,
            SettlementFulfillmentProof {
                kind: "mooglemail_sent_collected".to_owned(),
                reference,
                digest: [0x3c; 32],
                observed_at: Utc::now(),
            },
        )
        .await
        .map(|result| result.status)
}

/// COD 与载体付款：经投递 fence 建立冻结发送证明后，付款期内计为等待买家付款；寄出远超长期
/// 未领取阈值也不进入等待领取、积压或清单。买家付款领取后由领取证明满足，依赖解除，等待付款
/// 归零。
#[tokio::test]
async fn postgres_collection_cod_with_frozen_send_proof_waits_for_payment() {
    for payment in [Payment::EquipmentCod, Payment::CarrierCod] {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let now = Utc::now();
        let case = seed_case(pool, payment, now + Duration::days(9)).await;
        send_through_fence(&store, payment, &case).await;
        assert!(frozen_sent_proof(pool, case.delivery).await, "{payment:?} 冻结发送证明成立");

        // 8 天后观察：寄出时长远超 72 小时阈值，付款期仍未到。
        let later = now + Duration::days(8);
        let snapshot = observe(&store, later).await;
        let fulfillment = snapshot.fulfillment.expect("履约水位");
        // 载体付款的异世界交付受阻于付款，按既有口径计受阻；货款都按等待前置计。
        let blocked = i64::from(payment == Payment::CarrierCod);
        assert_eq!(
            grouping(&fulfillment),
            [1, 0, 0, blocked, 1, 0],
            "{payment:?}：等待付款 1，不进等待领取、积压或超期"
        );
        assert!(snapshot.collection_overdue_items.is_empty(), "{payment:?} 不进长期未领取清单");

        // 买家付款后领取：旧满足入口（未建立受保护资金账户）以 Sent 领取证明满足，依赖解除。
        assert_eq!(
            satisfy_collected(&store, &case).await.expect("领取证明满足付款义务"),
            SettlementFulfillmentReconcileStatus::Satisfied,
            "{payment:?}"
        );
        let snapshot = observe(&store, Utc::now()).await;
        let fulfillment = snapshot.fulfillment.expect("履约水位");
        assert_eq!(fulfillment.waiting_payment, 0, "{payment:?} 付款领取后不再等待付款");
        let unblocked: String = sqlx::query_scalar(
            "SELECT business_state FROM public.settlement_obligations WHERE id = $1",
        )
        .bind(match payment {
            Payment::EquipmentCod => case.payout,
            Payment::CarrierCod => stable_id(SEED, "isekai-winner_equipment_handoff"),
        })
        .fetch_one(pool)
        .await
        .expect("读取依赖义务");
        assert_eq!(unblocked, "ready", "{payment:?} 付款满足后解除直接依赖");

        pool.close().await;
    }
}

/// 存在受保护资金账户时，旧满足入口拒绝 COD 与载体付款：领取后必须经资金化校验，义务保持
/// 等待付款，不被旧路径直接满足。
#[tokio::test]
async fn postgres_collection_cod_collected_requires_funding_when_protected() {
    for payment in [Payment::EquipmentCod, Payment::CarrierCod] {
        let database = support::TestDatabase::create().await.expect("克隆模板库");
        let store = database.store().await.expect("以运行账号连接");
        let pool = store.postgres_pool();
        let now = Utc::now();
        let case = seed_case(pool, payment, now + Duration::days(1)).await;
        send_through_fence(&store, payment, &case).await;
        sqlx::query(
            "INSERT INTO public.settlement_funding_accounts (uid, status) VALUES ('7002', 'ready')",
        )
        .execute(pool)
        .await
        .expect("建立受保护资金账户");

        assert!(
            satisfy_collected(&store, &case).await.is_err(),
            "{payment:?} 受保护时旧满足入口必须拒绝"
        );
        let state: String = sqlx::query_scalar(
            "SELECT execution_state FROM public.settlement_obligation_fulfillments
             WHERE obligation_id = $1",
        )
        .bind(case.payment)
        .fetch_one(pool)
        .await
        .expect("读取付款履约");
        assert_eq!(state, "enqueued", "{payment:?} 付款义务未被旧路径满足");
        let snapshot = observe(&store, Utc::now()).await;
        assert_eq!(snapshot.fulfillment.map(|value| value.waiting_payment), Some(1));

        pool.close().await;
    }
}

/// 反例：付款投递已成功，但没有经投递 fence 留下发送尝试与 Sent 绑定，数据库不认冻结发送证明，
/// 付款义务不计为等待付款。
#[tokio::test]
async fn postgres_collection_cod_without_frozen_send_proof_is_not_waiting_payment() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let now = Utc::now();
    let case = seed_case(pool, Payment::EquipmentCod, now + Duration::days(1)).await;
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'succeeded', fencing_version = 1, attempt_count = 1,
             claimed_at = now(), sending_at = now(), completed_at = now(), updated_at = now(),
             external_reference = 'mooglemail:main:send:7400409',
             result_metadata = jsonb_build_object(
                 'operation', 'send', 'external_reference_proven', true,
                 'sent_recovered', true, 'proof', 'sent_recovered_frozen_v1')
         WHERE id = $1",
    )
    .bind(case.delivery)
    .execute(pool)
    .await
    .expect("写入缺少冻结证明的成功投递");
    assert!(!frozen_sent_proof(pool, case.delivery).await, "没有发送尝试与 Sent 绑定");
    let snapshot = observe(&store, now).await;
    let fulfillment = snapshot.fulfillment.expect("履约水位");
    assert_eq!(fulfillment.waiting_payment, 0, "缺少冻结证明不计为等待付款");
    assert_eq!(fulfillment.awaiting_collection, 0, "COD 不进入等待领取");
    assert_eq!(fulfillment.pending, 1, "按既有口径仍是待处理");

    pool.close().await;
}
