//! 恢复退件记录与 COD 重发的 PostgreSQL 18 用例。
//!
//! - 退件时间下界：收件箱时间只有分钟精度，recovery 创建时间却是亚秒级的结算时刻；下界取结算
//!   所在分钟再减 2 分钟时钟余量，余量内的合法退件必须被记录。仍早于下界的退件按证据不符隔离
//!   （消费、`evidence_mismatch` 阻断、不建动作、追加审计），队列继续处理后续退件。
//! - COD 重发：未达冻结上限的退回按 recovery 冻结的恢复策略版本从本次时刻重新起算付款窗口。
//!
//! 只用直接 SQL 构造事实，经 `RecoveryWorkStore` 与 `RecoveryResolutionStore` 端口驱动，不发
//! 外部请求。仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::recovery::{RecoveryResolutionStore, RecoveryWorkStore};
use auction_storage::DatabaseStore;
use chrono::{DateTime, Duration, DurationRound, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

/// 一笔主世界 COD 成交：买家、装备、已结算场次、lot 与三条结算义务。
struct CodSale {
    winner_user_id: Uuid,
    auction_id: Uuid,
    lot_id: Uuid,
    equipment_id: Uuid,
    external_equipment_id: String,
    payment_obligation: Uuid,
    seller_obligation: Uuid,
    refund_obligation: Uuid,
}

/// `recovery_version` 写入场次冻结快照，与 recovery 行的 `policy_version` 保持一致。
async fn insert_sale(
    pool: &PgPool,
    sequence: i64,
    at: DateTime<Utc>,
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

/// 已接受的 COD 退件；收件箱时间按调用方给出的分钟精度写入。
async fn insert_return_mail(
    pool: &PgPool,
    sale: &CodSale,
    external_mail_id: &str,
    received_at: DateTime<Utc>,
) -> Uuid {
    let mail_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at)
         VALUES ($1, 'recovery-returns', $2, 'main', 'delivery', $3, $4, 'accepted',
                 'settlement_recovery_returned', $5, $4 + interval '1 minute')",
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
    .execute(pool)
    .await
    .expect("写入退件");
    mail_id
}

/// 尚未收到退件的付款等待中 recovery；`created_at` 即结算发生时刻。
async fn insert_awaiting_recovery(
    pool: &PgPool,
    sale: &CodSale,
    created_at: DateTime<Utc>,
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
fn settlement_minute() -> (DateTime<Utc>, DateTime<Utc>) {
    let minute =
        (Utc::now() - Duration::hours(2)).duration_trunc(Duration::minutes(1)).expect("按分钟取整");
    (minute, minute + Duration::milliseconds(37_500))
}

async fn consumed_returns(pool: &PgPool, recovery_id: Uuid) -> i64 {
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
        .record_due_return(Utc::now())
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
        .record_due_return(Utc::now())
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
        .record_due_return(Utc::now())
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
        .record_due_return(Utc::now())
        .await
        .expect("后续合法退件必须照常记录")
        .expect("必须领取到后续退件");
    assert_eq!(recorded.recovery_id, valid_recovery.into());
    assert_eq!(recorded.return_mail_id, valid_mail);
    assert!(recorded.blocking_code.is_none());
    assert_eq!(recorded.action_kinds, vec!["reclaim_equipment_take".to_owned()]);
    assert!(blocked_return_audits(pool, valid_recovery).await.is_empty());
    assert!(
        store.record_due_return(Utc::now()).await.expect("队列排空不报错").is_none(),
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
        .record_due_return(Utc::now())
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
    let at = Utc::now() - Duration::hours(1);
    let sale = insert_sale(pool, sequence, at - Duration::hours(1), policy_version).await;
    let recovery_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let external_mail_id = format!("{}", 880_000 + sequence % 1_000);
    let digest = [6_u8; 32];
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference, created_at, updated_at, result_metadata)
         VALUES ($1, $2, decode(repeat('99', 32), 'hex'), 'settlement', $3, $4,
                 '{\"schema_version\":1,\"operation\":\"send\",\"world\":\"main\",\"template\":\"settlement_winner_equipment_cod\",\"credits\":0,\"cod\":100,\"attachment_ids\":[]}'::jsonb,
                 'succeeded', 2, 1, $5, $5, $5, $6, $5, $5, '{\"operation\":\"send\"}'::jsonb)",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(sale.payment_obligation.to_string())
    .bind(format!("recovery-return-player-{delivery_id}"))
    .bind(at - Duration::minutes(50))
    .bind(format!("mooglemail:main:send:{}", 770_000 + sequence % 1_000))
    .execute(pool)
    .await
    .expect("写入已发送的 COD 投递");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
             delivery_id, created_at, updated_at)
         VALUES ($1, 'enqueued', 2, 'main', 'send', $2, $3, $3)",
    )
    .bind(sale.payment_obligation)
    .bind(delivery_id)
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

async fn resend_due_cod(store: &DatabaseStore) -> (Uuid, DateTime<Utc>) {
    let now = Utc::now();
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
    assert!(store.advance_due_cod_return(Utc::now()).await.expect("重复调度不报错").is_none());

    for recovery_id in [seven_day, pre_season] {
        let (stage, deadline, return_mail_id, custody, count): (
            String,
            DateTime<Utc>,
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
