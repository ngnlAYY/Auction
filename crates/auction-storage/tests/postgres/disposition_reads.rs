//! W2.5 告警读侧口径切换、影子比对与 W2.6 履约终态统一处置读法的真实 PostgreSQL 验证。
//!
//! 只在 `test-db` feature 下编译；入口见 `just test-db`。测试只用直接 SQL 构造事实，
//! 不写业务命令、不发通知、不改动任何队列。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[allow(dead_code)]
use crate::support;

use std::sync::{Arc, Mutex};

use auction_application::operations::{
    AuditFailureMode, DispositionReadMode, OperationsAlertSnapshot, OperationsAlertSourceReader,
};
use auction_application::recovery::RecoveryResolutionStore;
use auction_application::runtime_switches::{RuntimeSwitchRole, RuntimeSwitches};
use auction_application::shadow_run::{ShadowOutcome, ShadowOwner, ShadowRecorder, ShadowSample};
use auction_storage::DatabaseStore;
use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

/// 捕获影子样本的测试记录器；记录器只累加，不改变调用点控制流。
#[derive(Default)]
struct CapturingShadow {
    samples: Mutex<Vec<ShadowSample>>,
}

impl ShadowRecorder for CapturingShadow {
    fn record(&self, sample: ShadowSample) {
        self.samples.lock().expect("capture").push(sample);
    }
}

impl CapturingShadow {
    fn samples(&self) -> Vec<ShadowSample> {
        self.samples.lock().expect("capture").clone()
    }
}

fn store_with_mode(store: &DatabaseStore, mode: DispositionReadMode) -> DatabaseStore {
    store.clone().with_runtime_switches(
        RuntimeSwitches::new(RuntimeSwitchRole::Worker, AuditFailureMode::Shadow, true)
            .with_disposition_read_mode_baseline(mode),
    )
}

async fn snapshot(store: &DatabaseStore) -> OperationsAlertSnapshot {
    store.snapshot(Utc::now()).await.expect("读取告警快照")
}

/// 插入一条 `origin_kind = 'mail_intake'` 的通知投递；`shape` 决定失败形态与处置。
enum DeliveryShape {
    /// 普通失败，没有任何处置事实：两个口径都必须计入。
    Plain,
    /// 关注提醒在开始发送前失去资格：两个口径都排除。
    LotWatchInvalidated,
    /// 操作者确认已处置：只有分类口径排除。
    Acknowledged,
}

async fn insert_delivery(pool: &PgPool, shape: DeliveryShape) -> (Uuid, i64) {
    let id = Uuid::new_v4();
    let fencing_version = 1_i64;
    let now = Utc::now() - chrono::Duration::minutes(5);
    let (origin_kind, origin_id, failure_code, result_metadata, sending_at) = match shape {
        DeliveryShape::Plain => (
            "mail_intake",
            format!("w2-5-plain:{id}"),
            "verified_not_sent",
            "{}".to_owned(),
            Some(now),
        ),
        DeliveryShape::LotWatchInvalidated => (
            "lot_watch",
            format!("w2-5-watch:{id}"),
            "lot_watch_invalidated",
            r#"{"outcome_unknown":false,"preflight_blocked":true,"reason_code":"lot_watch_invalidated"}"#
                .to_owned(),
            None,
        ),
        DeliveryShape::Acknowledged => (
            "mail_intake",
            format!("w2-5-ack:{id}"),
            "verified_not_sent",
            "{}".to_owned(),
            Some(now),
        ),
    };
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             created_at, updated_at, failure_code, result_metadata)
         VALUES ($1, $2, decode(repeat('33', 32), 'hex'), $3, $4, $5,
                 '{\"schema_version\":1,\"operation\":\"send\",\"template\":\"register_key\",\"credits\":0,\"cod\":0,\"attachment_ids\":[]}'::jsonb,
                 'failed', $6, 1, $7, $8, $7, $7, $7, $9, $10::jsonb)",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(origin_kind)
    .bind(origin_id)
    .bind(format!("player-{id}"))
    .bind(fencing_version)
    .bind(now)
    .bind(sending_at)
    .bind(failure_code)
    .bind(result_metadata)
    .execute(pool)
    .await
    .expect("写入通知投递");
    match shape {
        DeliveryShape::Plain => {}
        DeliveryShape::LotWatchInvalidated => {
            sqlx::query(
                "INSERT INTO public.delivery_dispositions
                    (delivery_id, fencing_version, kind, actor_source, actor_id,
                     reason_code, resolves_alert)
                 VALUES ($1, $2, 'preflight_retired', 'worker', 'delivery-worker',
                         'lot_watch_invalidated', true)",
            )
            .bind(id)
            .bind(fencing_version)
            .execute(pool)
            .await
            .expect("写入预检退役处置");
        }
        DeliveryShape::Acknowledged => {
            let audit_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO public.audit_records
                    (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
                     result, source, metadata)
                 VALUES ($1, now(), 'admin', 'admin', 'auction.delivery.acknowledge',
                         'delivery', $2, 'success', 'admin', '{}'::jsonb)",
            )
            .bind(audit_id)
            .bind(id.to_string())
            .execute(pool)
            .await
            .expect("写入确认审计");
            sqlx::query(
                "INSERT INTO public.delivery_dispositions
                    (delivery_id, fencing_version, kind, command_id, request_digest,
                     source_audit_id, actor_source, actor_id, reason, resolves_alert)
                 VALUES ($1, $2, 'operator_acknowledged', $3, decode(repeat('44', 32), 'hex'),
                         $4, 'admin', 'admin', 'history handled outside the outbox', true)",
            )
            .bind(id)
            .bind(fencing_version)
            .bind(Uuid::new_v4())
            .bind(audit_id)
            .execute(pool)
            .await
            .expect("写入确认处置");
        }
    }
    (id, fencing_version)
}

#[tokio::test]
async fn postgres_disposition_reads_modes_differ_only_by_the_switch() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    insert_delivery(pool, DeliveryShape::Plain).await;
    insert_delivery(pool, DeliveryShape::LotWatchInvalidated).await;
    insert_delivery(pool, DeliveryShape::Acknowledged).await;

    let classified = snapshot(&store_with_mode(&store, DispositionReadMode::Classified)).await;
    let classified_delivery = classified.delivery.expect("投递水位").failed;
    let classified_notification = classified.notification.expect("通知水位").failed_count;
    // 分类口径只计入普通失败：预检退役与操作者确认都已解除。
    assert_eq!(classified_delivery, 1);
    assert_eq!(classified_notification, 1);

    let legacy = snapshot(&store_with_mode(&store, DispositionReadMode::Legacy)).await;
    let legacy_delivery = legacy.delivery.expect("投递水位").failed;
    let legacy_notification = legacy.notification.expect("通知水位").failed_count;
    // 迁移前口径不认识 `operator_acknowledged`，因此多出那一笔；预检退役仍排除。
    assert_eq!(legacy_delivery, 2);
    assert_eq!(legacy_notification, 2);

    // 影子模式必须继续显示旧口径的裁决值，并逐项记录与分类口径的差异。
    let recorder = Arc::new(CapturingShadow::default());
    let shadow_store =
        store_with_mode(&store, DispositionReadMode::Shadow).with_shadow_recorder(recorder.clone());
    let shadow = snapshot(&shadow_store).await;
    assert_eq!(shadow.delivery.expect("投递水位").failed, legacy_delivery);
    assert_eq!(shadow.notification.expect("通知水位").failed_count, legacy_notification);

    let samples = recorder.samples();
    let sampled = |branch: &str| {
        samples
            .iter()
            .find(|sample| sample.owner == ShadowOwner::DispositionReads && sample.branch == branch)
            .unwrap_or_else(|| panic!("缺少分支 {branch} 的影子样本"))
    };
    assert_eq!(sampled("delivery_failed").outcome, ShadowOutcome::Mismatch);
    assert_eq!(sampled("notification_failed").outcome, ShadowOutcome::Mismatch);
    assert_eq!(sampled("notification_postage").outcome, ShadowOutcome::Match);
    assert_eq!(sampled("notification_oldest_failed_age").outcome, ShadowOutcome::Match);
    assert_eq!(sampled("notification_oldest_postage_age").outcome, ShadowOutcome::Match);
    assert_eq!(sampled("fulfillment_failed").outcome, ShadowOutcome::Match);

    // 回退只由开关触发：同一批事实在三种口径下的差异完全来自处置种类。
    let only_classified_store = store_with_mode(&store, DispositionReadMode::Classified);
    let again = snapshot(&only_classified_store).await;
    assert_eq!(again.delivery.expect("投递水位").failed, classified_delivery);
    store.postgres_pool().close().await;
}

/// 履约终态的种类；`CodReturned` 是当前仍缺少统一处置的终态。
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TerminalKind {
    /// 装备损失的线下补偿，且统一处置事实与终态在同一事务写入。
    LossWithDisposition,
    /// 同一终态但缺少处置事实（只有绕过写入口才能出现），用于证明读侧不会静默吞掉。
    LossWithoutDisposition,
    /// COD 三次退回后的终态：存在同一恢复记录的统一处置事实。
    CodReturnedWithDisposition,
}

/// 构造退款义务失败的履约终态；装备损失分支同时写入损失事实与（可选的）统一处置事实。
async fn insert_terminal(pool: &PgPool, uid: &str, kind: TerminalKind) {
    let now = Utc::now() - chrono::Duration::hours(1);
    let user_id = Uuid::new_v4();
    let auction_id = Uuid::new_v4();
    let lot_id = Uuid::new_v4();
    let equipment_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let payment_obligation = Uuid::new_v4();
    let seller_obligation = Uuid::new_v4();
    let refund_obligation = Uuid::new_v4();
    let recovery_id = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let loss_command_id = Uuid::new_v4();
    let is_loss =
        matches!(kind, TerminalKind::LossWithDisposition | TerminalKind::LossWithoutDisposition);

    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(format!("w2-6-user-{user_id}"))
        // 32 字节唯一摘要：同一测试库内多笔终态共用模板库，不能复用固定值。
        .bind({
            let mut hash = [0_u8; 32];
            hash[..16].copy_from_slice(user_id.as_bytes());
            hash[16..].copy_from_slice(user_id.as_bytes());
            hash
        })
        .execute(pool)
        .await
        .expect("写入测试用户");
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'main', $2, 'W2.6 test equipment', 'weapon', '{}'::jsonb, $3)",
    )
    .bind(equipment_id)
    .bind(format!("w2-6-eid-{equipment_id}"))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入测试装备");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', $2, 'W2.6 test auction', 'settled', $3, $3, $3, $3, $3, $3, $4)",
    )
    .bind(auction_id)
    .bind(i32::try_from(Uuid::new_v4().as_u128() % 1_000_000).expect("sequence"))
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
        "settlement": {"version": 2, "mode": "hold_curve_v2",
                       "compensation_pool_enabled": true, "anchors": []},
        "recovery": {"version": 2, "payment_timeout_seconds": 86400, "mode": "remind_v1"},
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
         VALUES ($1, 'w2-6-test', $2, $3, 'accepted')",
    )
    .bind(command_id)
    .bind(auction_id)
    .bind([6_u8; 32].as_slice())
    .execute(pool)
    .await
    .expect("写入测试命令");
    if is_loss {
        sqlx::query(
            "INSERT INTO public.lot_settlements
                (lot_id, end_price, listed_price, buyer_amount, seller_amount, tax_amount,
                 compensation_amount, hold_ppm, adjustment_ppm)
             VALUES ($1, 100, 90, 100, 90, 10, 0, 0, 0)",
        )
        .bind(lot_id)
        .execute(pool)
        .await
        .expect("写入 lot settlement");
        sqlx::query(
            "INSERT INTO public.settlement_funding_accounts (uid, status) VALUES ($1, 'ready')",
        )
        .bind(uid)
        .execute(pool)
        .await
        .expect("写入资金账号");
    }
    for (id, kind, state, amount, recipient_user, recipient_name, prerequisite) in [
        (payment_obligation, "winner_equipment_cod", "ready", 100, Some(user_id), None, None),
        (
            seller_obligation,
            "seller_payout",
            "ready",
            90,
            None,
            Some("W2.6 seller"),
            Some(payment_obligation),
        ),
        (
            refund_obligation,
            "buyer_refund",
            "contingent",
            100,
            Some(user_id),
            None,
            Some(payment_obligation),
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
        .bind(auction_id)
        .bind(lot_id)
        .bind(command_id)
        .bind(kind)
        .bind(state)
        .bind(recipient_user)
        .bind(recipient_name)
        .bind(amount)
        .bind(equipment_id)
        .bind(prerequisite)
        .execute(pool)
        .await
        .expect("写入义务");
    }
    if !is_loss {
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
                 last_error_code, completed_at, created_at, updated_at)
             VALUES ($1, 'failed', 2, 'main', 'send', 'cod_returned_three_times', $2, $2, $2)",
        )
        .bind(refund_obligation)
        .bind(now)
        .execute(pool)
        .await
        .expect("写入 COD 退回终态");
        if kind == TerminalKind::CodReturnedWithDisposition {
            sqlx::query(
                "INSERT INTO public.deliveries
                    (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                     status, fencing_version, attempt_count, claimed_at, completed_at, created_at,
                     updated_at, failure_code, result_metadata)
                 VALUES ($1, $2, decode(repeat('88', 32), 'hex'), 'settlement', $3, $4,
                         '{\"schema_version\":1,\"operation\":\"send\",\"template\":\"settlement_winner_equipment_cod\",\"credits\":0,\"cod\":0,\"attachment_ids\":[]}'::jsonb,
                         'failed', 2, 1, $5, $5, $5, $5, 'cod_returned_three_times', '{}'::jsonb)",
            )
            .bind(delivery_id)
            .bind(Uuid::new_v4())
            .bind(payment_obligation.to_string())
            .bind(format!("cod-return-player-{delivery_id}"))
            .bind(now)
            .execute(pool)
            .await
            .expect("写入 COD 投递");
            sqlx::query(
                "INSERT INTO public.settlement_recoveries
                    (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
                     payment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
                     policy_version, payment_deadline_at, stage, resolution, state_version,
                     created_at, updated_at, completed_at)
                 VALUES ($1, $2, $3, 'main', $4, $5, $6, $7, $8, 1, $9, 'completed',
                         'returned_after_three_cod_returns', 2, $9, $9, $9)",
            )
            .bind(recovery_id)
            .bind(auction_id)
            .bind(lot_id)
            .bind(equipment_id)
            .bind(user_id)
            .bind(payment_obligation)
            .bind(seller_obligation)
            .bind(refund_obligation)
            .bind(now)
            .execute(pool)
            .await
            .expect("写入 COD 恢复终态");
            sqlx::query(
                "INSERT INTO public.delivery_dispositions
                    (delivery_id, fencing_version, kind, settlement_recovery_id, actor_source,
                     actor_id, reason, reason_code, resolves_alert, detail)
                 VALUES ($1, 2, 'cod_returned', $2, 'worker', 'recovery-cod-return',
                         'COD returned to seller after three attempts',
                         'cod_returned_three_times', true,
                         jsonb_build_object('recovery_id', $2::text))",
            )
            .bind(delivery_id)
            .bind(recovery_id)
            .execute(pool)
            .await
            .expect("写入 COD 退回处置");
        }
        return;
    }
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, completed_at, created_at,
             updated_at, failure_code, result_metadata)
         VALUES ($1, $2, decode(repeat('55', 32), 'hex'), 'settlement', $3, $4,
                 '{\"schema_version\":1,\"operation\":\"send\",\"template\":\"settlement_payment_carrier\",\"credits\":0,\"cod\":0,\"attachment_ids\":[]}'::jsonb,
                 'failed', 2, 1, $5, $5, $5, $5, 'equipment_destroyed', '{}'::jsonb)",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(payment_obligation.to_string())
    .bind(format!("loss-player-{delivery_id}"))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入装备投递");
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
             policy_version, payment_deadline_at, stage, state_version, created_at, updated_at)
         VALUES ($1, $2, $3, 'main', $4, $5, $6, $7, $8, 1, $9, 'awaiting_payment', 2, $10, $10)",
    )
    .bind(recovery_id)
    .bind(auction_id)
    .bind(lot_id)
    .bind(equipment_id)
    .bind(user_id)
    .bind(payment_obligation)
    .bind(seller_obligation)
    .bind(refund_obligation)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("写入恢复事实");
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
            (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
             proof_kind, proof_reference, proof_digest, proof_observed_at, last_error_code,
             completed_at, created_at, updated_at)
         VALUES ($1, 'failed', 2, 'main', 'send', 'operator_equipment_loss_v1', $2,
                 decode(repeat('66', 32), 'hex'), $3, 'operator_external_buyer_compensation',
                 $3, $3, $3)",
    )
    .bind(refund_obligation)
    .bind(format!("operator_equipment_loss_v1:{loss_command_id}"))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入履约终态");
    sqlx::query(
        "INSERT INTO public.settlement_equipment_losses
            (command_id, lot_id, recovery_id, payment_obligation_id, delivery_id,
             seller_obligation_id, refund_obligation_id, uid, actor, reason,
             buyer_compensation_external, expected_recovery_version, expected_payment_version,
             expected_delivery_fencing_version, buyer_amount, compensation_amount, seller_amount,
             tax_amount, request_digest, original_snapshot, result)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'admin', 'w2-6 test loss', true, 2, 2, 1,
                 100, 0, 90, 10, decode(repeat('77', 32), 'hex'), '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(loss_command_id)
    .bind(lot_id)
    .bind(recovery_id)
    .bind(payment_obligation)
    .bind(delivery_id)
    .bind(seller_obligation)
    .bind(refund_obligation)
    .bind(uid)
    .execute(pool)
    .await
    .expect("写入损失事实");
    if kind == TerminalKind::LossWithDisposition {
        sqlx::query(
            "INSERT INTO public.delivery_dispositions
                (delivery_id, fencing_version, kind, equipment_loss_id, actor_source, actor_id,
                 reason, reason_code, resolves_alert, detail)
             VALUES ($1, 2, 'equipment_loss', $2, 'admin', 'admin', 'w2-6 test loss',
                     'equipment_destroyed', true,
                     jsonb_build_object('refund_obligation_id', $3::text))",
        )
        .bind(delivery_id)
        .bind(loss_command_id)
        .bind(refund_obligation.to_string())
        .execute(pool)
        .await
        .expect("写入装备损失处置");
    }
}

#[tokio::test]
async fn postgres_fulfillment_terminal_is_resolved_only_by_its_disposition() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    // 三笔履约终态各自独立：有处置的装备损失、缺处置的装备损失，以及 COD 三次退回。
    insert_terminal(pool, "90000001", TerminalKind::LossWithDisposition).await;
    insert_terminal(pool, "90000002", TerminalKind::LossWithoutDisposition).await;
    insert_terminal(pool, "90000004", TerminalKind::CodReturnedWithDisposition).await;
    insert_terminal(pool, "90000005", TerminalKind::CodReturnedWithDisposition).await;

    let classified = snapshot(&store_with_mode(&store, DispositionReadMode::Classified)).await;
    // 有统一处置事实的装备损失与两笔 COD 退回均已解除；只有缺处置的装备损失继续计入。
    assert_eq!(classified.fulfillment.expect("履约水位").failed, 1);

    let legacy = snapshot(&store_with_mode(&store, DispositionReadMode::Legacy)).await;
    // 迁移前口径只看损失事实：两笔装备损失都不计入，两笔 COD 退回终态仍然计入。
    assert_eq!(legacy.fulfillment.expect("履约水位").failed, 2);

    // 影子模式显示旧口径的裁决值，只把履约终态的差异记为可见样本。
    let recorder = Arc::new(CapturingShadow::default());
    let shadow = snapshot(
        &store_with_mode(&store, DispositionReadMode::Shadow)
            .with_shadow_recorder(recorder.clone()),
    )
    .await;
    assert_eq!(shadow.fulfillment.expect("履约水位").failed, 2);
    assert!(
        recorder.samples().iter().any(|sample| {
            sample.owner == ShadowOwner::DispositionReads
                && sample.branch == "fulfillment_failed"
                && sample.outcome == ShadowOutcome::Mismatch
        }),
        "履约终态差异必须进入影子样本"
    );

    store.postgres_pool().close().await;
}

/// 旧口径读不到时，Legacy/Shadow 不能把分类口径冒充成裁决值。
#[tokio::test]
async fn postgres_disposition_reads_paired_failure_never_shows_classified_authority() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    insert_delivery(pool, DeliveryShape::Plain).await;
    // 只破坏旧口径依赖的对象：分类口径（reads.rs）不引用历史自寄修复表。
    // DDL 夹具须由维护账号执行；被测读侧仍经运行账号。
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");
    sqlx::query("DROP TABLE public.settlement_legacy_self_reconciliations CASCADE")
        .execute(maintenance.postgres_pool())
        .await
        .expect("移除旧口径依赖");
    maintenance.postgres_pool().close().await;

    let classified = snapshot(&store_with_mode(&store, DispositionReadMode::Classified)).await;
    assert_eq!(
        classified.delivery.expect("分类投递水位").failed,
        1,
        "分类口径的配对读取失败后仍保留同一分类谓词的可见值"
    );

    for mode in [DispositionReadMode::Legacy, DispositionReadMode::Shadow] {
        let recorder = Arc::new(CapturingShadow::default());
        let value =
            snapshot(&store_with_mode(&store, mode).with_shadow_recorder(recorder.clone())).await;
        assert!(value.delivery.is_none(), "{} 必须报告投递来源不可读", mode.label());
        assert!(value.notification.is_none(), "{} 必须报告通知来源不可读", mode.label());
        assert!(value.fulfillment.is_none(), "{} 必须报告履约来源不可读", mode.label());
        if mode == DispositionReadMode::Shadow {
            assert!(
                recorder.samples().iter().all(|sample| {
                    sample.outcome == ShadowOutcome::Uncomparable
                        && sample.reason == Some("paired_query_error")
                }),
                "读取失败的影子样本必须记为不可比较"
            );
            assert_eq!(recorder.samples().len(), 6, "六个字段都必须留下不可比较样本");
        }
    }

    store.postgres_pool().close().await;
}

/// 构造真实第三次 COD 退回的可领取恢复事实，供 `advance_due_cod_return` 入口使用。
async fn insert_cod_reclaim(pool: &PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
    let now = Utc::now() - chrono::Duration::hours(1);
    let user_id = Uuid::new_v4();
    let auction_id = Uuid::new_v4();
    let lot_id = Uuid::new_v4();
    let equipment_id = Uuid::new_v4();
    let command_id = Uuid::new_v4();
    let settlement_command_id = Uuid::new_v4();
    let payment_obligation = Uuid::new_v4();
    let seller_obligation = Uuid::new_v4();
    let refund_obligation = Uuid::new_v4();
    let delivery_id = Uuid::new_v4();
    let recovery_id = Uuid::new_v4();
    let return_mail_id = Uuid::new_v4();

    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(format!("cod-reclaim-{user_id}"))
        .bind({
            let mut hash = [0_u8; 32];
            hash[..16].copy_from_slice(user_id.as_bytes());
            hash[16..].copy_from_slice(user_id.as_bytes());
            hash
        })
        .execute(pool)
        .await
        .expect("写入 COD 用户");
    sqlx::query(
        "INSERT INTO public.equipment
            (id, world_id, external_equipment_id, name, category, attributes, collected_at)
         VALUES ($1, 'main', $2, 'COD reclaim equipment', 'weapon', '{}'::jsonb, $3)",
    )
    .bind(equipment_id)
    .bind(format!("cod-reclaim-eid-{equipment_id}"))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入 COD 装备");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', $2, 'COD reclaim auction', 'settled', $3, $3, $3, $3, $3, $3, $4)",
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
            "version": 1,
            "mode": "hold_curve_v1",
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
    .expect("写入 COD 场次");
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
    .expect("写入 COD lot");
    for id in [command_id, settlement_command_id] {
        sqlx::query(
            "INSERT INTO public.auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code)
             VALUES ($1, 'cod-test', $2, $3, 'accepted')",
        )
        .bind(id)
        .bind(auction_id)
        .bind([3_u8; 32].as_slice())
        .execute(pool)
        .await
        .expect("写入 COD 命令");
    }
    for (id, kind, state, recipient_user, recipient_name, amount, prerequisite) in [
        (payment_obligation, "winner_equipment_cod", "ready", Some(user_id), None, 100, None),
        (
            seller_obligation,
            "seller_payout",
            "ready",
            None,
            Some("COD seller"),
            90,
            Some(payment_obligation),
        ),
        (
            refund_obligation,
            "buyer_refund",
            "contingent",
            Some(user_id),
            None,
            100,
            Some(payment_obligation),
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
        .bind(auction_id)
        .bind(lot_id)
        .bind(command_id)
        .bind(kind)
        .bind(state)
        .bind(recipient_user)
        .bind(recipient_name)
        .bind(amount)
        .bind(equipment_id)
        .bind(prerequisite)
        .execute(pool)
        .await
        .expect("写入 COD 义务");
    }
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, completed_at, created_at,
             updated_at, failure_code, result_metadata)
         VALUES ($1, $2, decode(repeat('99', 32), 'hex'), 'settlement', $3, $4,
                 '{\"schema_version\":1,\"operation\":\"send\",\"template\":\"settlement_winner_equipment_cod\",\"credits\":0,\"cod\":0,\"attachment_ids\":[]}'::jsonb,
                 'failed', 2, 1, $5, $5, $5, $5, 'cod_returned_three_times', '{}'::jsonb)",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(payment_obligation.to_string())
    .bind(format!("cod-reclaim-player-{delivery_id}"))
    .bind(now)
    .execute(pool)
    .await
    .expect("写入 COD 投递");
    for (obligation, delivery, error) in [
        (payment_obligation, Some(delivery_id), "cod_returned_three_times"),
        (seller_obligation, None, "equipment_returned_to_seller"),
    ] {
        sqlx::query(
            "INSERT INTO public.settlement_obligation_fulfillments
                (obligation_id, execution_state, state_version, dispatch_world, delivery_operation,
                 delivery_id, last_error_code, completed_at, created_at, updated_at)
             VALUES ($1, 'failed', 2, 'main', 'send', $2, $3, $4, $4, $4)",
        )
        .bind(obligation)
        .bind(delivery)
        .bind(error)
        .bind(now)
        .execute(pool)
        .await
        .expect("写入 COD fulfillment");
    }
    sqlx::query(
        "INSERT INTO public.auction_settlements
            (auction_id, command_id, policy_version, policy_mode, pool_enabled,
             opening_pool_balance, closing_pool_balance, tax_total, compensation_total,
             opening_pool_version, closing_pool_version, settled_at)
         VALUES ($1, $2, 1, 'hold_curve_v1', false, 0, 0, 0, 0, 1, 1, $3)",
    )
    .bind(auction_id)
    .bind(settlement_command_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("写入 COD 结算");
    sqlx::query(
        "INSERT INTO public.lot_settlements
            (lot_id, end_price, listed_price, buyer_amount, seller_amount, tax_amount,
             compensation_amount, hold_ppm, adjustment_ppm)
         VALUES ($1, 0, 0, 0, 0, 0, 0, 0, 0)",
    )
    .bind(lot_id)
    .execute(pool)
    .await
    .expect("写入 COD lot 结算");
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, processed_at)
         VALUES ($1, 'cod-reclaim', $2, 'main', 'delivery', $3, $4, 'accepted',
                 'accepted', $4)",
    )
    .bind(return_mail_id)
    .bind(format!("cod-reclaim-mid-{return_mail_id}"))
    .bind([4_u8; 32].as_slice())
    .bind(now)
    .execute(pool)
    .await
    .expect("写入 COD 退回邮件");
    sqlx::query(
        "INSERT INTO public.settlement_recoveries
            (id, auction_id, lot_id, world_id, equipment_id, winner_user_id,
             payment_obligation_id, seller_payout_obligation_id, buyer_refund_obligation_id,
             policy_version, payment_deadline_at, stage, state_version, return_mail_id,
             return_external_mail_id, custody_proof_kind, custody_proof_reference,
             custody_proof_digest, custody_proof_observed_at, cod_return_count,
             cod_return_limit, created_at, updated_at)
         VALUES ($1, $2, $3, 'main', $4, $5, $6, $7, $8, 1, $9, 'reclaiming', 1, $10,
                 $11, 'take_receipt', 'cod-reclaim-proof', decode(repeat('aa', 32), 'hex'),
                 $12, 3, 3, $13, $9)",
    )
    .bind(recovery_id)
    .bind(auction_id)
    .bind(lot_id)
    .bind(equipment_id)
    .bind(user_id)
    .bind(payment_obligation)
    .bind(seller_obligation)
    .bind(refund_obligation)
    .bind(now)
    .bind(return_mail_id)
    .bind(format!("cod-reclaim-return-{recovery_id}"))
    .bind(now - chrono::Duration::minutes(30))
    .bind(now - chrono::Duration::hours(2))
    .execute(pool)
    .await
    .expect("写入 COD 恢复");
    for (index, kind) in ["reclaim_equipment_take", "buyer_blacklist"].into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO public.settlement_recovery_actions
                (id, recovery_id, kind, ordinal, execution_state, state_version,
                 proof_kind, proof_reference, proof_digest, proof_observed_at,
                 created_at, updated_at, completed_at)
             VALUES ($1, $2, $3, $4, 'satisfied', 1, 'test_proof', $5, $6, $7, $7, $7, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(recovery_id)
        .bind(kind)
        .bind(i32::try_from(index).expect("ordinal"))
        .bind(format!("cod-reclaim-action-{index}-{recovery_id}"))
        .bind([5_u8; 32].as_slice())
        .bind(now)
        .execute(pool)
        .await
        .expect("写入 COD 恢复动作");
    }
    (recovery_id, payment_obligation, seller_obligation, delivery_id)
}

#[tokio::test]
async fn postgres_real_cod_return_writes_one_terminal_disposition_and_retry_is_idempotent() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let (recovery_id, payment_obligation, seller_obligation, delivery_id) =
        insert_cod_reclaim(pool).await;

    let advance = store
        .advance_due_cod_return(Utc::now())
        .await
        .expect("COD 第三次退回必须成功")
        .expect("必须领取到 COD 恢复");
    assert_eq!(advance.recovery_id, recovery_id.into());
    assert_eq!(advance.resolution.as_deref(), Some("returned_after_three_cod_returns"));

    let (stage, resolution): (String, Option<String>) =
        sqlx::query_as("SELECT stage, resolution FROM public.settlement_recoveries WHERE id = $1")
            .bind(recovery_id)
            .fetch_one(pool)
            .await
            .expect("读取 COD 恢复终态");
    assert_eq!(stage, "completed");
    assert_eq!(resolution.as_deref(), Some("returned_after_three_cod_returns"));

    for obligation in [payment_obligation, seller_obligation] {
        let state: String = sqlx::query_scalar(
            "SELECT execution_state FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
        )
        .bind(obligation)
        .fetch_one(pool)
        .await
        .expect("读取 COD 义务终态");
        assert_eq!(state, "failed");
    }

    let dispositions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.delivery_dispositions
         WHERE delivery_id = $1 AND kind = 'cod_returned' AND settlement_recovery_id = $2
           AND resolves_alert",
    )
    .bind(delivery_id)
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取 COD 处置");
    assert_eq!(dispositions, 1);

    // 真实写入链路也必须让分类读侧解除同一恢复记录的两个失败义务。
    let classified = snapshot(&store_with_mode(&store, DispositionReadMode::Classified)).await;
    assert_eq!(classified.fulfillment.expect("履约水位").failed, 0);

    // 已完成恢复不再被领取；重复调度不能追加第二条终态事实。
    assert!(store.advance_due_cod_return(Utc::now()).await.expect("重复调度不报错").is_none());
    let dispositions_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.delivery_dispositions
         WHERE delivery_id = $1 AND kind = 'cod_returned'",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("重复读取 COD 处置");
    assert_eq!(dispositions_after, 1);

    store.postgres_pool().close().await;
}
