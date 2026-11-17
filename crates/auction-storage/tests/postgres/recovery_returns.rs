//! 恢复退件记录与 COD 重发的 PostgreSQL 18 用例。
//!
//! - 退件时间下界：收件箱时间只有分钟精度，recovery 创建时间却是亚秒级的结算时刻；下界取结算
//!   所在分钟再减 2 分钟时钟余量，余量内的合法退件必须被记录。仍早于下界的退件按证据不符隔离
//!   （消费、`evidence_mismatch` 阻断、不建动作、追加审计），队列继续处理后续退件。
//! - COD 重发：未达冻结上限的退回按 recovery 冻结的恢复策略版本从本次时刻重新起算付款窗口。
//! - 自动重寄与退回卖家同样要求退件能把原 COD 每一次可能外达的发送逐一归属：截止之前另有一次
//!   可能外达的发送时以阻断码 `send_unattributed` 转人工，不重寄、不撤销成交，也不留在队头。
//! - 恢复流程接管退件之后，投递 Worker 的领取与发送围栏（零 Credits 的 COD 走无证据围栏）都不
//!   再放行原 COD；恢复 owner 自己的重寄清空当前退件与收回证明，照常领取与发送。
//!
//! 只用直接 SQL 构造事实，经 `RecoveryWorkStore`、`RecoveryResolutionStore` 与
//! `DeliveryWorkStore` 端口驱动，不发外部请求。仅在 `test-db` feature 下编译，入口见
//! `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendExpectation, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::recovery::{RecoveryResolutionStore, RecoveryWorkStore};
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

/// 一笔主世界 COD 成交：买家、装备、已结算场次、lot 与三条结算义务。
pub(crate) struct CodSale {
    pub(crate) winner_user_id: Uuid,
    pub(crate) auction_id: Uuid,
    pub(crate) lot_id: Uuid,
    pub(crate) equipment_id: Uuid,
    pub(crate) external_equipment_id: String,
    pub(crate) payment_obligation: Uuid,
    pub(crate) seller_obligation: Uuid,
    pub(crate) refund_obligation: Uuid,
}

/// `recovery_version` 写入场次冻结快照，与 recovery 行的 `policy_version` 保持一致。
pub(crate) async fn insert_sale(
    pool: &PgPool,
    sequence: i64,
    at: Timestamp,
    recovery_version: i32,
) -> CodSale {
    let payment_timeout_seconds = if recovery_version == 2 { 86_400 } else { 604_800 };
    let sale = CodSale {
        winner_user_id: Uuid::new_v4(),
        auction_id: Uuid::new_v4(),
        lot_id: Uuid::new_v4(),
        equipment_id: Uuid::new_v4(),
        external_equipment_id: format!("{}", 600_000 + sequence),
        payment_obligation: Uuid::new_v4(),
        seller_obligation: Uuid::new_v4(),
        refund_obligation: Uuid::new_v4(),
    };
    let command_id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(sale.winner_user_id)
        .bind(format!("recovery-return-{}", sale.winner_user_id))
        .bind({
            let mut hash = [0_u8; 32];
            hash[..16].copy_from_slice(sale.winner_user_id.as_bytes());
            hash[16..].copy_from_slice(sale.winner_user_id.as_bytes());
            hash
        })
        .execute(pool)
        .await
        .expect("写入买家");
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'main', $2, 'Recovery return equipment', 'weapon', '{}'::jsonb, $3)",
    )
    .bind(sale.equipment_id)
    .bind(&sale.external_equipment_id)
    .bind(at)
    .execute(pool)
    .await
    .expect("写入装备");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', $2, 'Recovery return auction', 'settled', $3, $3, $3, $3, $3, $3,
                 $4)",
    )
    .bind(sale.auction_id)
    .bind(sequence)
    .bind(at)
    .bind(json!({
        "schema_version": 5,
        "bidding_version": 2,
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
        "scheduled_end_at": at,
        "settlement": {"version": 1, "mode": "hold_curve_v1", "anchors": []},
        "recovery": {
            "version": recovery_version,
            "payment_timeout_seconds": payment_timeout_seconds,
            "mode": "remind_v1"
        },
    }))
    .execute(pool)
    .await
    .expect("写入场次");
    sqlx::query(
        "INSERT INTO public.lots
            (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
             effective_end_at, category_key, category_number)
         VALUES ($1, $2, $3, 1, 'sold', 100, 100, $4, 'weapon', 1)",
    )
    .bind(sale.lot_id)
    .bind(sale.auction_id)
    .bind(sale.equipment_id)
    .bind(at)
    .execute(pool)
    .await
    .expect("写入 lot");
    sqlx::query(
        "INSERT INTO public.auction_commands
            (command_id, command_kind, aggregate_id, request_hash, result_code)
         VALUES ($1, 'recovery-return-test', $2, $3, 'accepted')",
    )
    .bind(command_id)
    .bind(sale.auction_id)
    .bind([3_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入结算命令");
    for (id, kind, state, recipient_user, recipient_name, amount, prerequisite) in [
        (
            sale.payment_obligation,
            "winner_equipment_cod",
            "ready",
            Some(sale.winner_user_id),
            None,
            100,
            None,
        ),
        (
            sale.seller_obligation,
            "seller_payout",
            "ready",
            None,
            Some("Recovery seller"),
            90,
            Some(sale.payment_obligation),
        ),
        (
            sale.refund_obligation,
            "buyer_refund",
            "contingent",
            Some(sale.winner_user_id),
            None,
            100,
            Some(sale.payment_obligation),
        ),
    ] {
        sqlx::query(
            "INSERT INTO public.settlement_obligations
                (id, auction_id, lot_id, world_id, origin_command_id, policy_version,
                 obligation_kind, business_state, recipient_user_id, recipient_game_identity,
                 amount, carrier_credit, equipment_id, prerequisite_obligation_id)
             VALUES ($1, $2, $3, 'main', $4, 1, $5, $6, $7, $8, $9, 0, $10, $11)",
        )
        .bind(id)
        .bind(sale.auction_id)
        .bind(sale.lot_id)
        .bind(command_id)
        .bind(kind)
        .bind(state)
        .bind(recipient_user)
        .bind(recipient_name)
        .bind(amount)
        .bind(sale.equipment_id)
        .bind(prerequisite)
        .execute(pool)
        .await
        .expect("写入结算义务");
    }
    sale
}

/// 已接受的系统 COD 退件；收件箱时间按调用方给出的分钟精度写入。入站展示快照记录系统发件人
/// 与 `[RTS] ` + canonical 主题，恢复流程据此证明来源。
async fn insert_return_mail(
    pool: &PgPool,
    sale: &CodSale,
    external_mail_id: &str,
    received_at: Timestamp,
) -> Uuid {
    let mail_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at,
             sender, subject)
         VALUES ($1, 'recovery-returns', $2, 'main', 'delivery', $3, $4, 'accepted',
                 'settlement_recovery_returned', $5, $4 + interval '1 minute',
                 'MoogleMail', $6)",
    )
    .bind(mail_id)
    .bind(external_mail_id)
    .bind([4_u8; 32].as_slice())
    .bind(received_at)
    .bind(json!({
        "obligation_id": sale.payment_obligation.to_string(),
        "credits": 0,
        "cod": 0,
        "attachment_ids": [sale.external_equipment_id],
    }))
    .bind(format!("[RTS] Auction equipment delivery [{}]", sale.payment_obligation))
    .execute(pool)
    .await
    .expect("写入退件");
    mail_id
}

/// 尚未收到退件的付款等待中 recovery；`created_at` 即结算发生时刻。
pub(crate) async fn insert_awaiting_recovery(
    pool: &PgPool,
    sale: &CodSale,
    created_at: Timestamp,
) -> Uuid {
    let recovery_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
             policy_version, payment_deadline_at, stage, state_version, cod_return_count,
             cod_return_limit, created_at, updated_at)
         VALUES ($1, $2, $3, 'main', $4, $5, $6, $7, $8, 1, $9 + interval '7 days',
                 'awaiting_payment', 1, 0, 3, $9, $9)",
    )
    .bind(recovery_id)
    .bind(sale.auction_id)
    .bind(sale.lot_id)
    .bind(sale.equipment_id)
    .bind(sale.winner_user_id)
    .bind(sale.payment_obligation)
    .bind(sale.seller_obligation)
    .bind(sale.refund_obligation)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("写入付款等待中的 recovery");
    recovery_id
}

/// 结算时刻取在某分钟的第 37.5 秒，模拟亚秒级 `due_at`；返回该分钟起点与结算时刻。
pub(crate) fn settlement_minute() -> (Timestamp, Timestamp) {
    let minute = (Timestamp::now() - Duration::hours(2))
        .duration_trunc(Duration::minutes(1))
        .expect("按分钟取整");
    (minute, minute + Duration::milliseconds(37_500))
}

pub(crate) async fn consumed_returns(pool: &PgPool, recovery_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_recovery_return_consumptions WHERE recovery_id = $1",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取退件消费事实")
}

#[tokio::test]
async fn postgres_return_shown_in_the_settlement_minute_is_recorded() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_100_001, settled_at, 1).await;
    let recovery_id = insert_awaiting_recovery(pool, &sale, settled_at).await;
    // 收件箱只显示到分钟：与结算同一分钟的退件时间早于亚秒级的 created_at。
    let mail_id = insert_return_mail(pool, &sale, "990001", minute).await;

    let advance = store
        .record_due_return(Timestamp::now())
        .await
        .expect("同一分钟内的合法退件必须被记录")
        .expect("必须领取到该退件");
    assert_eq!(advance.recovery_id, recovery_id.into());
    assert_eq!(advance.return_mail_id, mail_id);
    assert_eq!(advance.stage, "reclaiming");
    assert!(advance.blocking_code.is_none(), "证据匹配的退件不得被阻断");
    assert_eq!(advance.action_kinds, vec!["reclaim_equipment_take".to_owned()]);

    assert_eq!(consumed_returns(pool, recovery_id).await, 1);
    let (stage, return_mail_id, count): (String, Option<Uuid>, i32) = sqlx::query_as(
        "SELECT stage, return_mail_id, cod_return_count
         FROM public.settlement_recoveries WHERE id = $1",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取 recovery");
    assert_eq!((stage.as_str(), return_mail_id, count), ("reclaiming", Some(mail_id), 1));
    let (state, source_mail_id, attachments): (String, Option<Uuid>, Vec<String>) = sqlx::query_as(
        "SELECT execution_state, source_mail_id, expected_attachment_ids
             FROM public.settlement_recovery_actions
             WHERE recovery_id = $1 AND kind = 'reclaim_equipment_take'",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取回收动作");
    assert_eq!(state, "pending");
    assert_eq!(source_mail_id, Some(mail_id));
    assert_eq!(attachments, vec![sale.external_equipment_id.clone()]);

    store.postgres_pool().close().await;
}

/// 结算所在分钟再往前 2 分钟仍在时钟余量内：退件照常记录并建立回收动作，不写隔离审计。
#[tokio::test]
async fn postgres_return_within_the_clock_slack_is_recorded() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_100_002, settled_at, 1).await;
    let recovery_id = insert_awaiting_recovery(pool, &sale, settled_at).await;
    let mail_id = insert_return_mail(pool, &sale, "990002", minute - Duration::minutes(2)).await;

    let advance = store
        .record_due_return(Timestamp::now())
        .await
        .expect("余量内的退件必须被记录")
        .expect("必须领取到该退件");
    assert_eq!(advance.recovery_id, recovery_id.into());
    assert_eq!(advance.return_mail_id, mail_id);
    assert!(advance.blocking_code.is_none(), "余量内的退件不得被阻断");
    assert_eq!(advance.action_kinds, vec!["reclaim_equipment_take".to_owned()]);
    assert_eq!(consumed_returns(pool, recovery_id).await, 1);
    assert!(blocked_return_audits(pool, recovery_id).await.is_empty(), "正常记录不写隔离审计");

    store.postgres_pool().close().await;
}

/// 早于时钟余量下界的退件不再以错误停住整个阶段：它被消费并以 `evidence_mismatch` 阻断转人工，
/// 不建自动动作，同事务追加带原因与证据的审计；排在其后的合法退件在下一次调用照常记录。
#[tokio::test]
async fn postgres_return_before_the_clock_slack_is_isolated_and_the_queue_advances() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let early_sale = insert_sale(pool, 7_100_003, settled_at, 1).await;
    let early_recovery = insert_awaiting_recovery(pool, &early_sale, settled_at).await;
    let early_received_at = minute - Duration::minutes(3);
    let early_mail = insert_return_mail(pool, &early_sale, "990003", early_received_at).await;
    let valid_sale = insert_sale(pool, 7_100_004, settled_at, 1).await;
    let valid_recovery = insert_awaiting_recovery(pool, &valid_sale, settled_at).await;
    let valid_mail = insert_return_mail(pool, &valid_sale, "990004", minute).await;

    // 队头是更早收到的越界退件：隔离后提交并返回摘要，而不是报错回滚。
    let isolated = store
        .record_due_return(Timestamp::now())
        .await
        .expect("越界退件必须隔离而不是报错")
        .expect("必须领取到越界退件");
    assert_eq!(isolated.recovery_id, early_recovery.into());
    assert_eq!(isolated.return_mail_id, early_mail);
    assert_eq!(isolated.stage, "reclaiming");
    assert_eq!(isolated.blocking_code.as_deref(), Some("evidence_mismatch"));
    assert!(isolated.action_kinds.is_empty(), "隔离的退件不得建立自动动作");
    assert_eq!(consumed_returns(pool, early_recovery).await, 1, "越界退件只消费一次");
    let (stage, blocking_code, return_mail_id, count): (String, Option<String>, Option<Uuid>, i32) =
        sqlx::query_as(
            "SELECT stage, blocking_code, return_mail_id, cod_return_count
             FROM public.settlement_recoveries WHERE id = $1",
        )
        .bind(early_recovery)
        .fetch_one(pool)
        .await
        .expect("读取隔离后的 recovery");
    assert_eq!(
        (stage.as_str(), blocking_code.as_deref(), return_mail_id, count),
        ("reclaiming", Some("evidence_mismatch"), Some(early_mail), 1)
    );
    let actions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_recovery_actions WHERE recovery_id = $1",
    )
    .bind(early_recovery)
    .fetch_one(pool)
    .await
    .expect("读取隔离 recovery 的动作");
    assert_eq!(actions, 0);
    let audits = blocked_return_audits(pool, early_recovery).await;
    assert_eq!(audits.len(), 1, "隔离只追加一条审计");
    assert_eq!(audits[0]["blocking_code"], "evidence_mismatch");
    assert_eq!(audits[0]["reason"], "received_outside_window");
    assert_eq!(audits[0]["return_mail_id"], early_mail.to_string());
    assert_eq!(audits[0]["return_received_at"], early_received_at.to_rfc3339());
    assert_eq!(audits[0]["earliest_received_at"], (minute - Duration::minutes(2)).to_rfc3339());

    // 队列继续推进：下一封合法退件照常记录并建立回收动作。
    let recorded = store
        .record_due_return(Timestamp::now())
        .await
        .expect("后续合法退件必须照常记录")
        .expect("必须领取到后续退件");
    assert_eq!(recorded.recovery_id, valid_recovery.into());
    assert_eq!(recorded.return_mail_id, valid_mail);
    assert!(recorded.blocking_code.is_none());
    assert_eq!(recorded.action_kinds, vec!["reclaim_equipment_take".to_owned()]);
    assert!(blocked_return_audits(pool, valid_recovery).await.is_empty());
    assert!(
        store.record_due_return(Timestamp::now()).await.expect("队列排空不报错").is_none(),
        "两封退件都已消费"
    );

    store.postgres_pool().close().await;
}

/// 收件时间在窗口内但附件与冻结预期不符：沿用既有隔离（消费、阻断、不建动作），审计原因标为
/// 内容不符，与时间越界区分。
#[tokio::test]
async fn postgres_return_with_mismatched_content_is_blocked_with_audit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_100_005, settled_at, 1).await;
    let recovery_id = insert_awaiting_recovery(pool, &sale, settled_at).await;
    let mail_id = insert_return_mail(pool, &sale, "990005", minute).await;
    sqlx::query(
        "UPDATE public.inbound_mail
         SET decision_metadata = jsonb_set(decision_metadata, '{attachment_ids}', '[\"1\"]')
         WHERE id = $1",
    )
    .bind(mail_id)
    .execute(pool)
    .await
    .expect("改写退件附件证据");

    let advance = store
        .record_due_return(Timestamp::now())
        .await
        .expect("内容不符的退件必须隔离而不是报错")
        .expect("必须领取到该退件");
    assert_eq!(advance.recovery_id, recovery_id.into());
    assert_eq!(advance.blocking_code.as_deref(), Some("evidence_mismatch"));
    assert!(advance.action_kinds.is_empty());
    assert_eq!(consumed_returns(pool, recovery_id).await, 1);
    let audits = blocked_return_audits(pool, recovery_id).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0]["reason"], "content_mismatch");
    assert_eq!(audits[0]["return_mail_id"], mail_id.to_string());

    store.postgres_pool().close().await;
}

/// 退件隔离审计的元数据，按发生顺序。
async fn blocked_return_audits(pool: &PgPool, recovery_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT metadata FROM public.audit_records
         WHERE entity_type = 'settlement_recovery' AND entity_id = $1
           AND action = 'recovery.return_blocked'
         ORDER BY occurred_at",
    )
    .bind(recovery_id.to_string())
    .fetch_all(pool)
    .await
    .expect("读取退件隔离审计")
}

/// 第 1 次退回已被取回（回收动作已满足）、等待重发 COD 的恢复事实；返回 recovery ID。
async fn insert_reclaimed_cod(pool: &PgPool, sequence: i64, policy_version: i32) -> Uuid {
    let at = Timestamp::now() - Duration::hours(1);
    let sale = insert_sale(pool, sequence, at - Duration::hours(1), policy_version).await;
    let recovery_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let external_mail_id = format!("{}", 880_000 + sequence % 1_000);
    let digest = [6_u8; 32];
    let prepare_command_id = Uuid::new_v4();
    let activate_command_id = Uuid::new_v4();
    // 投递与履约带完整 prepare/activate 身份：重寄后履约 Worker 能按生产路径重新领取它。
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata,
             activation_command_id, activation_request_digest, activated_at)
         VALUES ($1, $2, decode(repeat('99', 32), 'hex'), 'settlement', $3, $4,
                 '{\"schema_version\":1,\"operation\":\"send\",\"world\":\"main\",\"template\":\"settlement_winner_equipment_cod\",\"credits\":0,\"cod\":100,\"attachment_ids\":[]}'::jsonb,
                 'succeeded', 2, 1, $5, $5, $5, $6, $5, $5, '{\"operation\":\"send\"}'::jsonb,
                 $7, decode(repeat('98', 32), 'hex'), $5)",
    )
    .bind(delivery_id)
    .bind(prepare_command_id)
    .bind(sale.payment_obligation.to_string())
    .bind(format!("recovery-return-player-{delivery_id}"))
    .bind(at - Duration::minutes(50))
    .bind(format!("mooglemail:main:send:{}", 770_000 + sequence % 1_000))
    .bind(activate_command_id)
    .execute(pool)
    .await
    .expect("写入已发送的 COD 投递");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
             delivery_id, prepare_command_id, prepare_request_digest, activate_command_id,
             activate_request_digest, created_at, updated_at)
         VALUES ($1, 'enqueued', 2, 'main', 'send', $2, $3, decode(repeat('99', 32), 'hex'),
                 $4, decode(repeat('98', 32), 'hex'), $5, $5)",
    )
    .bind(sale.payment_obligation)
    .bind(delivery_id)
    .bind(prepare_command_id)
    .bind(activate_command_id)
    .bind(at - Duration::minutes(50))
    .execute(pool)
    .await
    .expect("写入 COD 履约");
    let mail_id =
        insert_return_mail(pool, &sale, &external_mail_id, at - Duration::minutes(40)).await;
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
             policy_version, payment_deadline_at, stage, state_version, return_mail_id,
             return_external_mail_id, custody_proof_kind, custody_proof_reference,
             custody_proof_digest, custody_proof_observed_at, cod_return_count,
             cod_return_limit, created_at, updated_at)
         VALUES ($1, $2, $3, 'main', $4, $5, $6, $7, $8, $9, $10, 'reclaiming', 3, $11,
                 $12, 'delivery_recovery_take_succeeded', $12, $13, $14, 1, 2, $15, $10)",
    )
    .bind(recovery_id)
    .bind(sale.auction_id)
    .bind(sale.lot_id)
    .bind(sale.equipment_id)
    .bind(sale.winner_user_id)
    .bind(sale.payment_obligation)
    .bind(sale.seller_obligation)
    .bind(sale.refund_obligation)
    .bind(policy_version)
    .bind(at)
    .bind(mail_id)
    .bind(&external_mail_id)
    .bind(digest.as_slice())
    .bind(at - Duration::minutes(10))
    .bind(at - Duration::hours(1))
    .execute(pool)
    .await
    .expect("写入回收中的 recovery");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_consumptions
            (return_mail_id, recovery_id, recorded_at) VALUES ($1, $2, $3)",
    )
    .bind(mail_id)
    .bind(recovery_id)
    .bind(at - Duration::minutes(30))
    .execute(pool)
    .await
    .expect("写入退件消费事实");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_actions
            (id, recovery_id, kind, ordinal, execution_state, state_version, source_mail_id,
             expected_attachment_ids, proof_kind, proof_reference, proof_digest,
             proof_observed_at, created_at, updated_at, completed_at)
         VALUES ($1, $2, 'reclaim_equipment_take', 1, 'satisfied', 2, $3, $4,
                 'delivery_recovery_take_succeeded', $5, $6, $7, $8, $7, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(recovery_id)
    .bind(mail_id)
    .bind(vec![sale.external_equipment_id.clone()])
    .bind(&external_mail_id)
    .bind(digest.as_slice())
    .bind(at - Duration::minutes(10))
    .bind(at - Duration::minutes(30))
    .execute(pool)
    .await
    .expect("写入已满足的回收动作");
    recovery_id
}

async fn resend_due_cod(store: &DatabaseStore) -> (Uuid, Timestamp) {
    let now = Timestamp::now();
    let advance = store
        .advance_due_cod_return(now)
        .await
        .expect("未达上限的退回必须重发 COD")
        .expect("必须领取到待重发的 recovery");
    assert_eq!(advance.stage, "awaiting_payment");
    assert!(advance.resolution.is_none());
    assert!(!advance.completed);
    (advance.recovery_id.as_uuid(), now)
}

#[tokio::test]
async fn postgres_cod_resend_restarts_the_frozen_payment_window() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let seven_day = insert_reclaimed_cod(pool, 7_200_001, 1).await;
    let pre_season = insert_reclaimed_cod(pool, 7_200_002, 2).await;

    let mut expected = std::collections::BTreeMap::new();
    for _ in 0..2 {
        let (recovery_id, now) = resend_due_cod(&store).await;
        let window = if recovery_id == seven_day {
            Duration::days(7)
        } else {
            assert_eq!(recovery_id, pre_season);
            Duration::hours(24)
        };
        expected.insert(recovery_id, (now + window).timestamp_micros());
    }
    assert!(
        store.advance_due_cod_return(Timestamp::now()).await.expect("重复调度不报错").is_none()
    );

    for recovery_id in [seven_day, pre_season] {
        let (stage, deadline, return_mail_id, custody, count): (
            String,
            Timestamp,
            Option<Uuid>,
            Option<String>,
            i32,
        ) = sqlx::query_as(
            "SELECT stage, payment_deadline_at, return_mail_id, custody_proof_kind,
                    cod_return_count
             FROM public.settlement_recoveries WHERE id = $1",
        )
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取重发后的 recovery");
        assert_eq!(stage, "awaiting_payment");
        assert_eq!(
            Some(deadline.timestamp_micros()),
            expected.get(&recovery_id).copied(),
            "付款窗口必须按冻结策略版本从本次时刻重新起算"
        );
        assert!(
            return_mail_id.is_none() && custody.is_none(),
            "重发必须清空当前退件指针与保管证明"
        );
        assert_eq!(count, 1, "重发不改写已记录的退回次数");
    }

    store.postgres_pool().close().await;
}

/// 重寄只改写恢复记录的当前期限。履约 Worker 重新领取重寄后的 COD 时，正文与载荷按首寄期限
/// D0（恢复记录创建时刻 + 冻结策略版本的付款期限）重建，当前期限只用于轮询聚焦，两者分别投影。
#[tokio::test]
async fn postgres_resent_cod_claim_projects_the_first_send_deadline_separately() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let seven_day = insert_reclaimed_cod(pool, 7_300_001, 1).await;
    let pre_season = insert_reclaimed_cod(pool, 7_300_002, 2).await;
    let mut current = std::collections::BTreeMap::new();
    for _ in 0..2 {
        let (recovery_id, now) = resend_due_cod(&store).await;
        current.insert(recovery_id, now);
    }

    let now = Timestamp::now();
    let claims = auction_application::settlement::SettlementFulfillmentStore::claim_due(
        &store,
        "a1-deadline-worker".to_owned(),
        now,
        now + Duration::minutes(5),
        10,
    )
    .await
    .expect("重寄后的 COD 履约必须可领取");
    assert_eq!(claims.len(), 2, "两笔重寄 COD 都应被领取：{claims:?}");
    for (recovery_id, window) in [(seven_day, Duration::days(7)), (pre_season, Duration::hours(24))]
    {
        let (payment_obligation, created_at, deadline): (Uuid, Timestamp, Timestamp) =
            sqlx::query_as(
                "SELECT payment_obligation_id, created_at, payment_deadline_at
                 FROM public.settlement_recoveries WHERE id = $1",
            )
            .bind(recovery_id)
            .fetch_one(pool)
            .await
            .expect("读取重寄后的 recovery");
        let claim = claims
            .iter()
            .find(|claim| claim.obligation_id.as_uuid() == payment_obligation)
            .expect("必须领取到该 recovery 的付款履约");
        assert_eq!(
            claim.initial_payment_deadline_at.map(auction_time::Timestamp::timestamp_micros),
            Some((created_at + window).timestamp_micros()),
            "正文期限必须是首寄期限 D0"
        );
        assert_eq!(
            claim.payment_deadline_at.map(auction_time::Timestamp::timestamp_micros),
            Some(deadline.timestamp_micros()),
            "轮询聚焦仍用重寄后重新起算的当前期限"
        );
        let resent_at = current.get(&recovery_id).copied().expect("重寄时刻");
        assert_eq!(deadline.timestamp_micros(), (resent_at + window).timestamp_micros());
        assert_ne!(claim.initial_payment_deadline_at, claim.payment_deadline_at);
    }

    store.postgres_pool().close().await;
}

/// 恢复记录的付款义务与原 COD 投递。
async fn payment_of(pool: &PgPool, recovery_id: Uuid) -> (Uuid, Uuid) {
    sqlx::query_as(
        "SELECT recovery.payment_obligation_id, fulfillment.delivery_id
         FROM public.settlement_recoveries AS recovery
         JOIN public.settlement_obligation_fulfillments AS fulfillment
           ON fulfillment.obligation_id = recovery.payment_obligation_id
         WHERE recovery.id = $1",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取付款义务与原 COD 投递")
}

/// 原 COD 投递在本轮退件截止之前另有一次可能外达的发送：命令身份不同的冻结发送尝试（fence 1），
/// 早于投递当前发送 5 分钟。
async fn add_earlier_send(pool: &PgPool, delivery_id: Uuid) {
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         SELECT id, 1, gen_random_uuid(), decode(repeat('9a', 32), 'hex'), payload_world,
                sending_at - interval '5 minutes',
                jsonb_build_object('world_id', payload_world, 'recipient', recipient,
                    'subject', 'Auction equipment delivery',
                    'body_digest', to_jsonb(array_fill(9, ARRAY[32])), 'credits', 0, 'cod', 100,
                    'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .execute(pool)
    .await
    .expect("写入截止之前的另一次发送");
}

/// 推进一条到期的 COD 退回，返回推进结果。
async fn advance(
    store: &DatabaseStore,
) -> auction_application::recovery::RecoveryResolutionAdvance {
    store
        .advance_due_cod_return(Timestamp::now())
        .await
        .expect("COD 退回推进不报错")
        .expect("领取到到期的 COD 退回")
}

/// 截止之前另有一次可能外达的发送：自动重寄（未达上限）与退回卖家（达到上限且已拉黑）都以
/// `send_unattributed` 阻断转人工并提交，原投递不重置、付款履约不收口、不建寄回义务、不追加处置；
/// 阻断后不再留在队头。
#[tokio::test]
async fn postgres_cod_advance_blocks_returns_that_cannot_be_attributed() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let resend = insert_reclaimed_cod(pool, 7_400_001, 1).await;
    let to_seller = insert_reclaimed_cod(pool, 7_400_002, 1).await;
    // 达到冻结上限且拉黑已完成：本应撤销成交并寄回卖家。
    sqlx::query("UPDATE public.settlement_recoveries SET cod_return_count = 2 WHERE id = $1")
        .bind(to_seller)
        .execute(pool)
        .await
        .expect("退回次数达到上限");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_actions
            (id, recovery_id, kind, ordinal, execution_state, state_version, proof_kind,
             proof_reference, proof_digest, proof_observed_at, reason, created_at, updated_at,
             completed_at)
         SELECT gen_random_uuid(), recovery.id, 'buyer_blacklist', 2, 'satisfied', 2,
                'users_blacklist_applied', recovery.winner_user_id::text,
                decode(repeat('9b', 32), 'hex'), recovery.updated_at, 'COD returned repeatedly',
                recovery.updated_at, recovery.updated_at, recovery.updated_at
         FROM public.settlement_recoveries AS recovery WHERE recovery.id = $1",
    )
    .bind(to_seller)
    .execute(pool)
    .await
    .expect("写入已满足的拉黑动作");
    for recovery_id in [resend, to_seller] {
        add_earlier_send(pool, payment_of(pool, recovery_id).await.1).await;
    }

    for _ in 0..2 {
        let blocked = advance(&store).await;
        assert_eq!((blocked.stage.as_str(), blocked.resolution), ("reclaiming", None));
        assert!(!blocked.completed);
    }
    assert!(
        store.advance_due_cod_return(Timestamp::now()).await.expect("排空不报错").is_none(),
        "阻断后的恢复记录不再被自动推进领取"
    );

    for recovery_id in [resend, to_seller] {
        let (stage, resolution, blocking_code, return_mail): (
            String,
            Option<String>,
            Option<String>,
            Option<Uuid>,
        ) = sqlx::query_as(
            "SELECT stage, resolution, blocking_code, return_mail_id
             FROM public.settlement_recoveries WHERE id = $1",
        )
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取阻断后的恢复记录");
        assert_eq!((stage.as_str(), resolution), ("reclaiming", None));
        assert_eq!(blocking_code.as_deref(), Some("send_unattributed"));
        assert!(return_mail.is_some(), "当前退件与收回证明保留，供人工核对");
        let (payment, delivery) = payment_of(pool, recovery_id).await;
        let (status, fence, sending_at): (String, i64, Option<Timestamp>) = sqlx::query_as(
            "SELECT status, fencing_version, sending_at FROM public.deliveries WHERE id = $1",
        )
        .bind(delivery)
        .fetch_one(pool)
        .await
        .expect("读取原 COD 投递");
        assert_eq!((status.as_str(), fence), ("succeeded", 2), "原投递不重置、不重寄");
        assert!(sending_at.is_some());
        let state: String = sqlx::query_scalar(
            "SELECT execution_state FROM public.settlement_obligation_fulfillments
             WHERE obligation_id = $1",
        )
        .bind(payment)
        .fetch_one(pool)
        .await
        .expect("读取付款履约");
        assert_eq!(state, "enqueued", "付款履约不收口、不重排");
        let (dispositions, returns, audits): (i64, i64, i64) = sqlx::query_as(
            "SELECT
                (SELECT count(*) FROM public.delivery_dispositions WHERE delivery_id = $1),
                (SELECT count(*) FROM public.settlement_obligations AS obligation
                 JOIN public.settlement_recoveries AS recovery ON recovery.lot_id = obligation.lot_id
                 WHERE recovery.id = $2 AND obligation.obligation_kind = 'withdrawn_return'),
                (SELECT count(*) FROM public.audit_records
                 WHERE entity_type = 'settlement_recovery' AND entity_id = $2::text
                   AND action = 'recovery.send_attribution_blocked')",
        )
        .bind(delivery)
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取处置、寄回义务与阻断审计");
        assert_eq!((dispositions, returns, audits), (0, 0, 1));
    }

    store.postgres_pool().close().await;
}

/// 原 COD 在退件到达前经 Admin 重试改回 pending（被退回的那次发送留作冻结尝试），恢复接管后投递
/// 门禁把它一直留在待发：它当前 fence 从未进入发送围栏，是确定未外写的意图，不再阻断归属，自动
/// 重寄以新 fence 接替它（审计 `superseded_queued_intent=true`），不以重寄失败的错误占住队头。
/// 反例：原 COD 已进入发送围栏（`sending`）时外部写入未解决，仍以 `send_unattributed` 阻断（原因
/// `send_in_flight`）并提交，其后到期的恢复记录照常重寄。
#[tokio::test]
async fn postgres_cod_resend_supersedes_a_queued_original_but_not_a_sending_one() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let queued = insert_reclaimed_cod(pool, 7_400_201, 1).await;
    let in_flight = insert_reclaimed_cod(pool, 7_400_203, 1).await;
    let next = insert_reclaimed_cod(pool, 7_400_202, 1).await;
    for recovery_id in [queued, in_flight] {
        let (_, delivery) = payment_of(pool, recovery_id).await;
        sqlx::query(
            "INSERT INTO public.delivery_send_attempts
                (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
                 expectation)
             SELECT id, fencing_version, command_id, request_digest, payload_world, sending_at,
                    jsonb_build_object('world_id', payload_world, 'recipient', recipient,
                        'subject', 'Auction equipment delivery',
                        'body_digest', to_jsonb(array_fill(9, ARRAY[32])), 'credits', 0,
                        'cod', 100, 'equipment_ids', '[]'::jsonb, 'credential', 'null'::jsonb)
             FROM public.deliveries WHERE id = $1",
        )
        .bind(delivery)
        .execute(pool)
        .await
        .expect("被退回的那次发送留作冻结尝试");
        sqlx::query(
            "UPDATE public.deliveries
             SET status = 'pending', sending_at = NULL, claimed_at = NULL, completed_at = NULL,
                 external_reference = NULL, fencing_version = fencing_version + 1
             WHERE id = $1",
        )
        .bind(delivery)
        .execute(pool)
        .await
        .expect("原 COD 改回待发");
    }
    let (_, sending_delivery) = payment_of(pool, in_flight).await;
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'sending', claimed_at = now(), sending_at = now(),
             lease_token = gen_random_uuid(), lease_expires_at = now() + interval '5 minutes'
         WHERE id = $1",
    )
    .bind(sending_delivery)
    .execute(pool)
    .await
    .expect("原 COD 已进入发送围栏");

    let resent = advance(&store).await;
    assert_eq!(resent.recovery_id.as_uuid(), queued, "更早到期的恢复记录先推进");
    assert_eq!((resent.stage.as_str(), resent.resolution), ("awaiting_payment", None));
    let (_, queued_delivery) = payment_of(pool, queued).await;
    let (status, fence): (String, i64) =
        sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id = $1")
            .bind(queued_delivery)
            .fetch_one(pool)
            .await
            .expect("读取原 COD 投递");
    assert_eq!((status.as_str(), fence), ("pending", 4), "重寄以新 fence 接替待发意图");
    let superseded: bool = sqlx::query_scalar(
        "SELECT (metadata->>'superseded_queued_intent')::boolean FROM public.audit_records
         WHERE entity_type = 'delivery' AND entity_id = $1 AND action = 'delivery.cod_return_resend'",
    )
    .bind(queued_delivery.to_string())
    .fetch_one(pool)
    .await
    .expect("读取重寄审计");
    assert!(superseded);

    let blocked = advance(&store).await;
    assert_eq!(blocked.recovery_id.as_uuid(), in_flight);
    assert_eq!((blocked.stage.as_str(), blocked.resolution), ("reclaiming", None));
    let (blocking_code, reason): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT recovery.blocking_code,
                (SELECT audit.metadata->>'reason' FROM public.audit_records AS audit
                 WHERE audit.entity_type = 'settlement_recovery'
                   AND audit.entity_id = recovery.id::text
                   AND audit.action = 'recovery.send_attribution_blocked')
         FROM public.settlement_recoveries AS recovery WHERE recovery.id = $1",
    )
    .bind(in_flight)
    .fetch_one(pool)
    .await
    .expect("读取阻断码与原因");
    assert_eq!(blocking_code.as_deref(), Some("send_unattributed"));
    assert_eq!(reason.as_deref(), Some("send_in_flight"));
    let resent = advance(&store).await;
    assert_eq!(
        (resent.recovery_id.as_uuid(), resent.stage.as_str()),
        (next, "awaiting_payment"),
        "队头阻断之后其他恢复记录照常重寄"
    );

    store.postgres_pool().close().await;
}

/// 领取一轮到期投递。
async fn claim_deliveries(store: &DatabaseStore) -> Vec<DeliveryWorkClaim> {
    let now = Timestamp::now();
    let grace = StdDuration::from_secs(60);
    DeliveryWorkStore::begin_claim_cycle(store, now, grace).await.expect("领取前清理");
    DeliveryWorkStore::claim_due(
        store,
        "recovery-return-gate".to_owned(),
        now,
        now + Duration::minutes(2),
        grace,
        DeliveryClaimExclusions::default(),
        20,
    )
    .await
    .expect("领取投递")
}

/// 零 Credits 的 COD 经无证据发送围栏（不走现金授权）进入 sending。
async fn fence_cod(store: &DatabaseStore, claim: &DeliveryWorkClaim) -> DeliveryFenceOutcome {
    let expectation = DeliveryPreparedExpectation::Send(Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: "Auction equipment delivery".to_owned(),
        body_digest: [0x9c; 32],
        credits: 0,
        cod: 100,
        equipment_ids: Vec::new(),
        credential: None,
    }));
    store
        .start_sending_authorized(claim.clone(), Timestamp::now(), None, Some(&expectation))
        .await
        .expect("发送围栏必须确定结束")
}

/// 恢复 owner 的重寄照常领取与发送；随后又有来源可证明的系统退件到达（尚未消费）时，已领取的
/// 原 COD 在发送围栏被拒绝（不冻结发送尝试、不进入 sending），释放后也不再被领取。对照组同库
/// 照常进入 sending，证明拒绝不是发送节奏所致。
#[tokio::test]
async fn postgres_delivery_worker_never_sends_a_cod_whose_return_recovery_owns() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let owned = insert_reclaimed_cod(pool, 7_400_101, 1).await;
    let control = insert_reclaimed_cod(pool, 7_400_102, 1).await;
    for _ in 0..2 {
        resend_due_cod(&store).await;
    }
    let (owned_payment, owned_delivery) = payment_of(pool, owned).await;
    let (_, control_delivery) = payment_of(pool, control).await;

    let claims = claim_deliveries(&store).await;
    let claim_of = |delivery: Uuid| {
        claims
            .iter()
            .find(|claim| claim.delivery_id == delivery)
            .cloned()
            .expect("恢复 owner 重寄的原 COD 照常领取")
    };
    let owned_claim = claim_of(owned_delivery);
    let control_claim = claim_of(control_delivery);

    // 领取之后，买家再次拒收：系统退件到达，恢复流程尚未消费。
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender, subject)
         SELECT gen_random_uuid(), earlier.mailbox_key, '889101', earlier.world_id,
                earlier.mail_kind, earlier.content_digest, clock_timestamp(), earlier.status,
                earlier.decision_code, earlier.decision_metadata, clock_timestamp(),
                earlier.sender, earlier.subject
         FROM public.inbound_mail AS earlier
         WHERE earlier.decision_metadata->>'obligation_id' = $1::text",
    )
    .bind(owned_payment)
    .execute(pool)
    .await
    .expect("写入尚未消费的系统退件");

    assert!(
        matches!(fence_cod(&store, &owned_claim).await, DeliveryFenceOutcome::Deferred),
        "恢复流程已接管退件，通用路径不能再发送原 COD"
    );
    let (status, attempts): (String, i64) = sqlx::query_as(
        "SELECT status,
                (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(owned_delivery)
    .fetch_one(pool)
    .await
    .expect("读取被拒绝的投递");
    assert_eq!((status.as_str(), attempts), ("claimed", 0), "拒绝时不冻结发送尝试");
    assert!(
        matches!(fence_cod(&store, &control_claim).await, DeliveryFenceOutcome::Sending),
        "对照组照常进入 sending"
    );

    DeliveryWorkStore::release(&store, owned_claim, Timestamp::now())
        .await
        .expect("释放未开始的领取");
    assert!(
        claim_deliveries(&store).await.iter().all(|claim| claim.delivery_id != owned_delivery),
        "退件到达之后原 COD 不再被领取"
    );

    store.postgres_pool().close().await;
}
