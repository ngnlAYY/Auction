//! 结算退件消费的 PostgreSQL 18 用例。
//!
//! - 只认系统来源：无前缀主题或非系统发件人的历史退件由结算退件消费方隔离为 `untrusted`，
//!   恢复流程不消费、不计次数；同一义务的系统 `[RTS]` 退件仍由恢复流程照常记录。
//! - 货款被退回：只追加证据并把未终结履约转人工对账（`settlement_mail_returned`），不改投递、
//!   不写资金返还、不新建投递；已终结履约只留证；无法绑定的退件只隔离；队列按收件顺序推进。
//!
//! 只用直接 SQL 构造事实，经 `RecoveryWorkStore` 端口以运行账号驱动，不发外部请求。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{SettlementReturnMatch, SettlementReturnReason};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::recovery_returns::{
    CodSale, consumed_returns, insert_awaiting_recovery, insert_sale, settlement_minute,
};
use crate::support;

mod alerts;
mod cases;
mod counts;
mod flows;
mod races;
mod recheck;
mod sweep;

/// 写入一封已接受的结算退件证据；发件人与主题缺失时模拟入站展示快照之前的历史行。
pub(crate) async fn insert_inbound(
    pool: &PgPool,
    external_mail_id: &str,
    received_at: DateTime<Utc>,
    header: Option<(&str, String)>,
    metadata: Value,
) -> Uuid {
    let mail_id = Uuid::new_v4();
    let (sender, subject) =
        header.map_or((None, None), |(sender, subject)| (Some(sender), Some(subject)));
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at,
             sender, subject)
         VALUES ($1, 'settlement-returns', $2, 'main', 'delivery', $3, $4, 'accepted',
                 'settlement_recovery_returned', $5, $4 + interval '1 minute', $6, $7)",
    )
    .bind(mail_id)
    .bind(external_mail_id)
    .bind([5_u8; 32].as_slice())
    .bind(received_at)
    .bind(metadata)
    .bind(sender)
    .bind(subject)
    .execute(pool)
    .await
    .expect("写入结算退件证据");
    mail_id
}

pub(crate) fn evidence(obligation: Uuid, credits: i64, attachments: &[&str]) -> Value {
    json!({
        "obligation_id": obligation.to_string(),
        "credits": credits,
        "cod": 0,
        "attachment_ids": attachments,
    })
}

/// 为义务写入一条已寄出的结算 Send 投递与对应履约；`state` 为履约执行状态。
pub(crate) async fn insert_sent_fulfillment(
    pool: &PgPool,
    obligation: Uuid,
    template: &str,
    credits: i64,
    sending_at: DateTime<Utc>,
    state: &str,
) -> Uuid {
    let delivery_id = Uuid::new_v4();
    let prepare_command_id = Uuid::new_v4();
    let activate_command_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('91', 32), 'hex'), 'settlement', $3, 'Recovery seller',
                 jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'main',
                     'template', $4::text, 'variables', jsonb_build_object('reference', $3),
                     'credits', $5::bigint, 'cod', 0, 'equipment_secret_refs', '[]'::jsonb),
                 'succeeded', 1, 1, $6, $6, $6, $7, $6, $6, '{\"operation\":\"send\"}'::jsonb,
                 $8, decode(repeat('92', 32), 'hex'), $6)",
    )
    .bind(delivery_id)
    .bind(prepare_command_id)
    .bind(obligation.to_string())
    .bind(template)
    .bind(credits)
    .bind(sending_at)
    .bind(format!("mooglemail:main:send:{}", 660_000_000 + delivery_id.as_u128() % 1_000_000))
    .bind(activate_command_id)
    .execute(pool)
    .await
    .expect("写入已寄出的结算投递");
    let satisfied = state == "satisfied";
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
             delivery_id, prepare_command_id, prepare_request_digest, activate_command_id,
             activate_request_digest, proof_kind, proof_reference, proof_digest,
             proof_observed_at, completed_at, created_at, updated_at)
         VALUES ($1, $2, 3, 'main', 'send', $3, $4, decode(repeat('91', 32), 'hex'),
                 $5, decode(repeat('92', 32), 'hex'),
                 CASE WHEN $6 THEN 'mooglemail_sent_collected' END,
                 CASE WHEN $6 THEN '1234567' END,
                 CASE WHEN $6 THEN decode(repeat('93', 32), 'hex') END,
                 CASE WHEN $6 THEN $7::timestamptz END,
                 CASE WHEN $2 IN ('satisfied', 'failed', 'manual_reconcile')
                      THEN $7::timestamptz END,
                 $7, $7)",
    )
    .bind(obligation)
    .bind(state)
    .bind(delivery_id)
    .bind(prepare_command_id)
    .bind(activate_command_id)
    .bind(satisfied)
    .bind(sending_at)
    .execute(pool)
    .await
    .expect("写入结算履约");
    delivery_id
}

async fn evidence_row(
    pool: &PgPool,
    mail_id: Uuid,
) -> (String, String, Option<Uuid>, Option<Uuid>, Option<String>) {
    sqlx::query_as(
        "SELECT match_state, reason_code, obligation_id, delivery_id, previous_execution_state
         FROM public.settlement_return_evidence WHERE return_mail_id = $1",
    )
    .bind(mail_id)
    .fetch_one(pool)
    .await
    .expect("读取退件证据")
}

async fn fulfillment_state(pool: &PgPool, obligation: Uuid) -> (String, Option<String>, i64) {
    sqlx::query_as(
        "SELECT execution_state, last_error_code, state_version
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(obligation)
    .fetch_one(pool)
    .await
    .expect("读取履约状态")
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned())).fetch_one(pool).await.expect("读取计数")
}

/// 来源无法证明的退件（无前缀主题、玩家发件人、缺展示快照的历史行）由结算退件消费方隔离，
/// 恢复流程不消费、不计次数、不改变恢复状态；同一义务的系统 `[RTS]` 退件仍由恢复流程记录，
/// 两边分区互斥。
#[tokio::test]
async fn postgres_unproven_returns_are_isolated_without_touching_recovery() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale: CodSale = insert_sale(pool, 7_400_001, settled_at, 1).await;
    let recovery_id = insert_awaiting_recovery(pool, &sale, settled_at).await;
    let assets = [sale.external_equipment_id.as_str()];
    let prefixless = insert_inbound(
        pool,
        "991001",
        minute + Duration::minutes(5),
        Some(("BuyerPlayer", format!("Auction equipment delivery [{}]", sale.payment_obligation))),
        evidence(sale.payment_obligation, 0, &assets),
    )
    .await;
    let legacy = insert_inbound(
        pool,
        "991002",
        minute + Duration::minutes(6),
        None,
        evidence(sale.payment_obligation, 0, &assets),
    )
    .await;

    assert!(
        store.record_due_return(Utc::now()).await.expect("恢复流程不报错").is_none(),
        "来源无法证明的退件不能进入恢复流程"
    );
    for expected in [prefixless, legacy] {
        let consumed = store
            .consume_due_settlement_return(Utc::now())
            .await
            .expect("隔离不报错")
            .expect("必须消费到不可信退件");
        assert_eq!(consumed.return_mail_id, expected);
        assert_eq!(consumed.decision.matched, SettlementReturnMatch::Untrusted);
        assert_eq!(consumed.decision.reason, SettlementReturnReason::SourceUnproven);
        assert_eq!(
            evidence_row(pool, expected).await,
            (
                "untrusted".to_owned(),
                "source_unproven".to_owned(),
                Some(sale.payment_obligation),
                None,
                None
            )
        );
    }
    assert!(store.consume_due_settlement_return(Utc::now()).await.expect("排空").is_none());
    let (stage, count_value): (String, i32) = sqlx::query_as(
        "SELECT stage, cod_return_count FROM public.settlement_recoveries WHERE id = $1",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取 recovery");
    assert_eq!((stage.as_str(), count_value), ("awaiting_payment", 0), "隔离不计次数");
    assert_eq!(consumed_returns(pool, recovery_id).await, 0);

    // 同一义务的系统退件只属于恢复流程。
    let system = insert_inbound(
        pool,
        "991003",
        minute + Duration::minutes(7),
        Some((
            "MoogleMail",
            format!("[RTS] Auction equipment delivery [{}]", sale.payment_obligation),
        )),
        evidence(sale.payment_obligation, 0, &assets),
    )
    .await;
    assert!(store.consume_due_settlement_return(Utc::now()).await.expect("不报错").is_none());
    let recorded = store
        .record_due_return(Utc::now())
        .await
        .expect("系统退件必须被恢复流程记录")
        .expect("必须领取到系统退件");
    assert_eq!(recorded.return_mail_id, system);
    assert!(recorded.blocking_code.is_none());
    assert_eq!(consumed_returns(pool, recovery_id).await, 1);

    store.postgres_pool().close().await;
}

/// 卖方货款被退回只留证、转人工对账；原投递、资金与 outbox 都不变，重复消费不再发生。
#[tokio::test]
async fn postgres_returned_payout_is_held_for_manual_reconcile_without_money_movement() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_400_002, settled_at, 1).await;
    let sent_at = minute + Duration::minutes(10) + Duration::seconds(17);
    let delivery_id = insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        90,
        sent_at,
        "enqueued",
    )
    .await;
    let deliveries_before = count(pool, "SELECT count(*) FROM public.deliveries").await;
    let mail_id = insert_inbound(
        pool,
        "991101",
        minute + Duration::minutes(40),
        Some(("MoogleMail", format!("[RTS] Auction seller payout [{}]", sale.seller_obligation))),
        evidence(sale.seller_obligation, 90, &[]),
    )
    .await;

    assert!(store.record_due_return(Utc::now()).await.expect("恢复流程不报错").is_none());
    let consumed = store
        .consume_due_settlement_return(Utc::now())
        .await
        .expect("货款退件必须被消费")
        .expect("必须领取到货款退件");
    assert_eq!(consumed.return_mail_id, mail_id);
    assert_eq!(consumed.obligation_id, Some(sale.seller_obligation));
    assert_eq!(consumed.delivery_id, Some(delivery_id));
    assert_eq!(consumed.decision.matched, SettlementReturnMatch::ManualReconcile);
    assert_eq!(consumed.decision.reason, SettlementReturnReason::ReturnedAssetsMatch);
    assert_eq!(
        evidence_row(pool, mail_id).await,
        (
            "manual_reconcile".to_owned(),
            "returned_assets_match".to_owned(),
            Some(sale.seller_obligation),
            Some(delivery_id),
            Some("enqueued".to_owned())
        )
    );
    assert_eq!(
        fulfillment_state(pool, sale.seller_obligation).await,
        ("manual_reconcile".to_owned(), Some("settlement_mail_returned".to_owned()), 4)
    );
    let (status, fencing): (String, i64) =
        sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_one(pool)
            .await
            .expect("读取原投递");
    assert_eq!((status.as_str(), fencing), ("succeeded", 1), "原投递不改写");
    assert_eq!(count(pool, "SELECT count(*) FROM public.deliveries").await, deliveries_before);
    assert_eq!(count(pool, "SELECT count(*) FROM public.settlement_funding_returns").await, 0);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE entity_type = 'settlement_obligation' AND entity_id = $1
           AND action = 'auction.fulfillment.manual_reconcile'
           AND metadata->>'return_mail_id' = $2",
    )
    .bind(sale.seller_obligation.to_string())
    .bind(mail_id.to_string())
    .fetch_one(pool)
    .await
    .expect("读取转人工审计");
    assert_eq!(audits, 1);
    assert!(store.consume_due_settlement_return(Utc::now()).await.expect("排空").is_none());

    store.postgres_pool().close().await;
}

/// 已终结履约只留证；未知义务、主题与种类不符、资产不符都不阻塞后续退件，队列按收件顺序
/// 逐封消费。
#[tokio::test]
async fn postgres_terminal_and_unbindable_returns_are_recorded_in_order() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_400_003, settled_at, 1).await;
    let sent_at = minute + Duration::minutes(5);
    let returned_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_obligations
            (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
             obligation_kind, business_state, recipient_game_identity, amount, carrier_credit,
             equipment_id)
         SELECT $1, auction_id, lot_id, 'main', origin_command_id, 1, 'withdrawn_return', 'ready',
                'Recovery seller', 0, 0, equipment_id
         FROM public.settlement_obligations WHERE id = $2",
    )
    .bind(returned_id)
    .bind(sale.seller_obligation)
    .execute(pool)
    .await
    .expect("写入撤拍寄回义务");
    insert_sent_fulfillment(
        pool,
        returned_id,
        "settlement_equipment_handoff",
        0,
        sent_at,
        "satisfied",
    )
    .await;
    insert_sent_fulfillment(
        pool,
        sale.seller_obligation,
        "settlement_seller_payout",
        90,
        sent_at,
        "enqueued",
    )
    .await;
    let unknown_obligation = Uuid::new_v4();
    let mails = [
        insert_inbound(
            pool,
            "991201",
            minute + Duration::minutes(20),
            Some(("MoogleMail", format!("[RTS] Auction equipment delivery [{returned_id}]"))),
            evidence(returned_id, 0, &[sale.external_equipment_id.as_str()]),
        )
        .await,
        insert_inbound(
            pool,
            "991202",
            minute + Duration::minutes(21),
            Some(("MoogleMail", format!("[RTS] Auction buyer refund [{unknown_obligation}]"))),
            evidence(unknown_obligation, 90, &[]),
        )
        .await,
        insert_inbound(
            pool,
            "991203",
            minute + Duration::minutes(22),
            Some((
                "MoogleMail",
                format!("[RTS] Auction buyer refund [{}]", sale.seller_obligation),
            )),
            evidence(sale.seller_obligation, 90, &[]),
        )
        .await,
        insert_inbound(
            pool,
            "991204",
            minute + Duration::minutes(23),
            Some((
                "MoogleMail",
                format!("[RTS] Auction seller payout [{}]", sale.seller_obligation),
            )),
            evidence(sale.seller_obligation, 1, &[]),
        )
        .await,
    ];
    let expected = [
        ("fulfillment_terminal", "returned_assets_match", Some(returned_id), Some("satisfied")),
        ("unbound", "obligation_unknown", None, None),
        ("unbound", "subject_kind_mismatch", Some(sale.seller_obligation), None),
        (
            "manual_reconcile",
            "returned_assets_mismatch",
            Some(sale.seller_obligation),
            Some("enqueued"),
        ),
    ];
    for (mail_id, (state, reason, obligation, previous)) in mails.iter().zip(expected) {
        let consumed = store
            .consume_due_settlement_return(Utc::now())
            .await
            .expect("消费不报错")
            .expect("必须按收件顺序领取");
        assert_eq!(consumed.return_mail_id, *mail_id);
        let (stored_state, stored_reason, stored_obligation, delivery, stored_previous) =
            evidence_row(pool, *mail_id).await;
        assert_eq!(
            (stored_state.as_str(), stored_reason.as_str(), stored_obligation),
            (state, reason, obligation)
        );
        assert_eq!(stored_previous.as_deref(), previous);
        assert_eq!(delivery.is_some(), previous.is_some());
    }
    assert!(store.consume_due_settlement_return(Utc::now()).await.expect("排空").is_none());
    let (returned_state, _, _) = fulfillment_state(pool, returned_id).await;
    assert_eq!(returned_state, "satisfied", "已满足事实不改写");
    assert_eq!(fulfillment_state(pool, sale.seller_obligation).await.0, "manual_reconcile");

    store.postgres_pool().close().await;
}
