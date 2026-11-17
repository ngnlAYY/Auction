//! 确认端把恢复记录已消费的退件逐一归属到原义务可能已到达远端的发送（PostgreSQL 18）。
//!
//! 时间截止只能排除退件之后的发送：同一投递在截止之前有两次可能外达的发送、同一分钟内多次发送，
//! 或时钟偏差让退件之后的重发落在截止之前，都会通过截止判断，而其中一封邮件可能仍在买家手里。
//! 收回、取消与装备收回的确认要求可能外达的发送与已消费的系统退件计数相等、且当前退件之后没有
//! 任何发送，否则以 `SendUnattributed` 失败关闭，什么都不改。只有投递当前这次发送能凭投递行证明
//! 确定没有外写（`failed` 且明确记录 `write_started=false`、`outcome_unknown=false`）；缺键、null
//! 或错误类型都按可能已外达计数。多轮 COD 重寄时每轮一次发送对应一封已消费的退件，照常确认。

use auction_application::core::ApplicationError;
use auction_application::recovery::{
    RecoveryCommandError, RecoveryCommandRepository, RecoveryConfirmationAction,
};
use auction_storage::DatabaseStore;
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use super::recovered::fixture::insert_recovered_sale;
use super::{closure_audits, confirmation, dispositions, fulfillment};
use crate::support;

/// 夹具里恢复记录的创建时刻；夹具的发送、退件与消费时刻都相对它偏移（原付款 +5 分钟发送，
/// 当前退件 +30 分钟收到、+31 分钟被消费，截止为 +31 分钟）。
pub(super) async fn created_at(pool: &PgPool, recovery_id: Uuid) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT created_at FROM public.settlement_recoveries WHERE id = $1")
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取恢复记录创建时刻")
}

/// 给投递追加一条冻结发送尝试（指定 fence，命令身份与投递当前发送不同，因此不是当前尝试）；
/// 不改投递行。
pub(super) async fn insert_attempt(
    pool: &PgPool,
    delivery_id: Uuid,
    fence: i64,
    sending_at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         SELECT id, $2, gen_random_uuid(), decode(repeat('f1', 32), 'hex'),
                payload_world, $3,
                jsonb_build_object('world_id', payload_world, 'recipient', recipient,
                    'subject', 'Recovery binding', 'body_digest', to_jsonb(array_fill(7, ARRAY[32])),
                    'credits', 0, 'cod', 0, 'equipment_ids', '[]'::jsonb,
                    'credential', 'null'::jsonb)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .bind(fence)
    .bind(sending_at)
    .execute(pool)
    .await
    .expect("写入冻结发送尝试");
}

/// 把投递当前发送移到 `sending_at`、fence 改为 `fence`，并按投递自身的命令、摘要、世界与收件人
/// 冻结同一次发送，使它成为 `delivery_current_send_attempt` 解析出的当前尝试。
pub(super) async fn freeze_current_attempt(
    pool: &PgPool,
    delivery_id: Uuid,
    fence: i64,
    sending_at: DateTime<Utc>,
) {
    sqlx::query("UPDATE public.deliveries SET fencing_version = $2, sending_at = $3 WHERE id = $1")
        .bind(delivery_id)
        .bind(fence)
        .bind(sending_at)
        .execute(pool)
        .await
        .expect("移动当前发送");
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         SELECT id, fencing_version, command_id, request_digest, payload_world, sending_at,
                jsonb_build_object('world_id', payload_world, 'recipient', recipient,
                    'subject', 'Recovery binding', 'body_digest', to_jsonb(array_fill(7, ARRAY[32])),
                    'credits', 0, 'cod', 0, 'equipment_ids', '[]'::jsonb,
                    'credential', 'null'::jsonb)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .execute(pool)
    .await
    .expect("冻结当前发送");
}

/// 模拟退件之后经旧 Admin 缺口重试并再次寄出成功：fence 推进、发送围栏落在 `sending_at`，投递
/// 仍是成功，并留下同一 fence 的冻结发送尝试。
async fn resend_after_return(pool: &PgPool, delivery_id: Uuid, sending_at: DateTime<Utc>) {
    let fence: i64 =
        sqlx::query_scalar("SELECT fencing_version + 1 FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_one(pool)
            .await
            .expect("读取投递 fence");
    sqlx::query(
        "UPDATE public.deliveries SET status = 'succeeded', completed_at = $2 WHERE id = $1",
    )
    .bind(delivery_id)
    .bind(sending_at)
    .execute(pool)
    .await
    .expect("模拟退件之后的再次寄出");
    freeze_current_attempt(pool, delivery_id, fence, sending_at).await;
}

/// 把恢复记录当前退件的收件时刻移到 `received_at`（消费时刻不变）。
async fn move_return_receipt(pool: &PgPool, recovery_id: Uuid, received_at: DateTime<Utc>) {
    sqlx::query(
        "UPDATE public.inbound_mail SET received_at = $2
         WHERE id = (SELECT return_mail_id FROM public.settlement_recoveries WHERE id = $1)",
    )
    .bind(recovery_id)
    .bind(received_at)
    .execute(pool)
    .await
    .expect("移动退件收件时刻");
}

/// 追加一封更早轮次的系统退件及其消费事实（同一付款义务、同一恢复记录，收件后 1 分钟消费），
/// 模拟 COD 重寄之前已经收回的那一轮。
async fn add_consumed_return(
    pool: &PgPool,
    sale: &Sale,
    external_mail_id: &str,
    received_at: DateTime<Utc>,
) {
    let mail = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at, sender, subject)
         SELECT $1, 'recovery-binding', $2, 'main', 'delivery', decode(repeat('b7', 32), 'hex'),
                $3, 'accepted', 'settlement_recovery_returned', current.decision_metadata, $3,
                'MoogleMail', current.subject
         FROM public.inbound_mail AS current
         WHERE current.id = (SELECT return_mail_id FROM public.settlement_recoveries
                             WHERE id = $4)",
    )
    .bind(mail)
    .bind(external_mail_id)
    .bind(received_at)
    .bind(sale.recovery_id)
    .execute(pool)
    .await
    .expect("写入更早轮次的系统退件");
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_consumptions
            (return_mail_id, recovery_id, recorded_at) VALUES ($1, $2, $3)",
    )
    .bind(mail)
    .bind(sale.recovery_id)
    .bind(received_at + Duration::minutes(1))
    .execute(pool)
    .await
    .expect("写入更早轮次的消费事实");
}

/// 把投递当前这次发送改为终态失败，结果元数据由调用方给出（远端引用清空）。
async fn fail_current_send(pool: &PgPool, delivery_id: Uuid, metadata: serde_json::Value) {
    sqlx::query(
        "UPDATE public.deliveries
         SET status = 'failed', failure_code = 'send_rejected', external_reference = NULL,
             result_metadata = $2
         WHERE id = $1",
    )
    .bind(delivery_id)
    .bind(metadata)
    .execute(pool)
    .await
    .expect("当前发送终态失败");
}

async fn recovery_state(pool: &PgPool, recovery_id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT stage, resolution FROM public.settlement_recoveries WHERE id = $1")
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取恢复记录")
}

/// 确认失败关闭：返回明确的 `SendUnattributed`，恢复记录、付款履约、处置与收口审计都保持原样。
async fn assert_rejected(store: &DatabaseStore, sale: &Sale, action: RecoveryConfirmationAction) {
    let pool = store.postgres_pool();
    let payment_before = fulfillment(pool, sale.payment).await;
    let seller_before = fulfillment(pool, sale.seller).await;
    let error = store
        .confirm_recovery(confirmation(sale, action))
        .await
        .expect_err("退件不能唯一归属到一次发送时不能确认");
    assert!(
        matches!(error, ApplicationError::RecoveryCommand(RecoveryCommandError::SendUnattributed)),
        "{action:?}: {error:?}"
    );
    assert_eq!(recovery_state(pool, sale.recovery_id).await, ("reclaiming".to_owned(), None));
    assert_eq!(fulfillment(pool, sale.payment).await, payment_before);
    assert_eq!(fulfillment(pool, sale.seller).await, seller_before);
    assert!(dispositions(pool, sale.payment_delivery).await.is_empty());
    assert_eq!(closure_audits(pool, sale).await, 0);
}

/// 正例：确认照常收口原付款，在原投递当前 fence 上恰好一条处置。
async fn assert_confirmed(store: &DatabaseStore, sale: &Sale, action: RecoveryConfirmationAction) {
    let pool = store.postgres_pool();
    store.confirm_recovery(confirmation(sale, action)).await.expect("退件可以逐一归属时照常确认");
    assert_eq!(recovery_state(pool, sale.recovery_id).await.1.as_deref(), Some(action.as_str()));
    assert_eq!(dispositions(pool, sale.payment_delivery).await.len(), 1);
}

async fn voidable_sale(store: &DatabaseStore, world: &'static str, sequence: i64) -> Sale {
    insert_reclaiming_sale(
        store.postgres_pool(),
        SaleSpec {
            world,
            sequence,
            cod_return_count: if world == "main" { 3 } else { 1 },
            cod_return_limit: 3,
            blacklist_satisfied: world == "main",
        },
    )
    .await
}

/// 收回（main）与取消（isekai）：退件消费之后又寄出、仅有退件之后的冻结发送尝试、或发送落在退件
/// 收件之后与消费之前，确认都拒绝；退件之前恰好一次、且就是当前发送的冻结尝试照常放行。
#[tokio::test]
async fn postgres_voiding_confirmation_rejects_sends_after_the_return() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    for (world, base, action) in [
        ("main", 7_900_001_i64, RecoveryConfirmationAction::Reclaimed),
        ("isekai", 7_900_011, RecoveryConfirmationAction::Cancelled),
    ] {
        let resent = voidable_sale(&store, world, base).await;
        let at = created_at(pool, resent.recovery_id).await;
        resend_after_return(pool, resent.payment_delivery, at + Duration::minutes(50)).await;
        assert_rejected(&store, &resent, action).await;

        let attempt_only = voidable_sale(&store, world, base + 1).await;
        let at = created_at(pool, attempt_only.recovery_id).await;
        insert_attempt(pool, attempt_only.payment_delivery, 3, at + Duration::minutes(40)).await;
        assert_rejected(&store, &attempt_only, action).await;

        let before_consumption = voidable_sale(&store, world, base + 2).await;
        let at = created_at(pool, before_consumption.recovery_id).await;
        move_return_receipt(pool, before_consumption.recovery_id, at + Duration::minutes(20)).await;
        insert_attempt(pool, before_consumption.payment_delivery, 3, at + Duration::minutes(25))
            .await;
        assert_rejected(&store, &before_consumption, action).await;

        // 正例：退件之前恰好一次发送，冻结尝试就是当前发送。
        let returned = voidable_sale(&store, world, base + 3).await;
        let at = created_at(pool, returned.recovery_id).await;
        freeze_current_attempt(pool, returned.payment_delivery, 2, at + Duration::minutes(5)).await;
        assert_confirmed(&store, &returned, action).await;
    }

    store.postgres_pool().close().await;
}

/// 截止之前的发送也必须能唯一归属：同一投递截止前两次发送、同分钟内多次发送、时钟偏差让退件之后
/// 的重发落在截止之前，都是两次可能外达的发送对应一封退件，确认拒绝。只有投递当前发送明确记录
/// 未开始写出且结果非未知时不计入；缺键、null 或字符串都按可能已外达。
#[tokio::test]
async fn postgres_voiding_confirmation_rejects_unattributable_sends_before_the_cutoff() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    for (world, base, action) in [
        ("main", 7_900_201_i64, RecoveryConfirmationAction::Reclaimed),
        ("isekai", 7_900_221, RecoveryConfirmationAction::Cancelled),
    ] {
        // 截止前同一投递两次发送：更早的一次（fence 1）与当前发送（fence 2）都可能外达。
        let twice = voidable_sale(&store, world, base).await;
        let at = created_at(pool, twice.recovery_id).await;
        insert_attempt(pool, twice.payment_delivery, 1, at + Duration::minutes(2)).await;
        freeze_current_attempt(pool, twice.payment_delivery, 2, at + Duration::minutes(5)).await;
        assert_rejected(&store, &twice, action).await;

        // 同分钟多次发送：两次都落在退件收件那一分钟、早于截止，时间无法区分。
        let same_minute = voidable_sale(&store, world, base + 1).await;
        let at = created_at(pool, same_minute.recovery_id).await;
        let minute = at + Duration::minutes(30);
        insert_attempt(pool, same_minute.payment_delivery, 1, minute + Duration::seconds(10)).await;
        freeze_current_attempt(
            pool,
            same_minute.payment_delivery,
            2,
            minute + Duration::seconds(40),
        )
        .await;
        assert_rejected(&store, &same_minute, action).await;

        // 时钟偏差：退件之后的重发被记在退件收件分钟之前，截止判断放行，计数仍是两次。
        let skewed = voidable_sale(&store, world, base + 2).await;
        let at = created_at(pool, skewed.recovery_id).await;
        insert_attempt(pool, skewed.payment_delivery, 1, at + Duration::minutes(5)).await;
        resend_after_return(
            pool,
            skewed.payment_delivery,
            at + Duration::minutes(29) + Duration::seconds(30),
        )
        .await;
        assert_rejected(&store, &skewed, action).await;

        // 当前发送的失败结果缺键、为 null 或为字符串：不能证明未外写，同样拒绝。
        for (offset, metadata) in [
            (3, json!({"operation": "send", "write_started": false})),
            (4, json!({"operation": "send", "write_started": false, "outcome_unknown": null})),
            (5, json!({"operation": "send", "write_started": "false", "outcome_unknown": false})),
        ] {
            let unproven = voidable_sale(&store, world, base + offset).await;
            let at = created_at(pool, unproven.recovery_id).await;
            insert_attempt(pool, unproven.payment_delivery, 1, at + Duration::minutes(5)).await;
            freeze_current_attempt(pool, unproven.payment_delivery, 2, at + Duration::minutes(10))
                .await;
            fail_current_send(pool, unproven.payment_delivery, metadata).await;
            assert_rejected(&store, &unproven, action).await;
        }

        // 正例：更早的一次已外达并被退回；当前发送明确未开始写出且结果非未知，不计入。
        let unsent = voidable_sale(&store, world, base + 6).await;
        let at = created_at(pool, unsent.recovery_id).await;
        insert_attempt(pool, unsent.payment_delivery, 1, at + Duration::minutes(5)).await;
        freeze_current_attempt(pool, unsent.payment_delivery, 2, at + Duration::minutes(10)).await;
        fail_current_send(
            pool,
            unsent.payment_delivery,
            json!({"operation": "send", "write_started": false, "outcome_unknown": false}),
        )
        .await;
        assert_confirmed(&store, &unsent, action).await;
    }

    store.postgres_pool().close().await;
}

/// 多轮 COD 重寄：每轮一次发送对应一封已消费的系统退件，计数相等照常收回；某一轮多出一次发送，
/// 或第一轮之前看不到任何发送（旧版本没有冻结尝试、又被重寄清掉的发送）都拒绝。
#[tokio::test]
async fn postgres_reclaim_confirmation_attributes_each_cod_round() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    for (sequence, variant) in [(7_900_301_i64, 0), (7_900_302, 1), (7_900_303, 2)] {
        let sale = voidable_sale(&store, "main", sequence).await;
        let at = created_at(pool, sale.recovery_id).await;
        // 第 1 轮：+2 分钟发送，+8 分钟退回；第 2 轮：+12 分钟重寄，+18 分钟退回；第 3 轮是当前
        // 发送（+25 分钟）与夹具的当前退件（+30 分钟收到、+31 分钟消费）。
        for (prefix, minutes) in [(640_000, 8), (650_000, 18)] {
            let mid = format!("{}", prefix + sequence % 1_000);
            add_consumed_return(pool, &sale, &mid, at + Duration::minutes(minutes)).await;
        }
        // 变体 2 去掉第 1 轮的发送，模拟旧版本没有冻结尝试、又被重寄清掉的那一次。
        if variant != 2 {
            insert_attempt(pool, sale.payment_delivery, 1, at + Duration::minutes(2)).await;
        }
        insert_attempt(pool, sale.payment_delivery, 3, at + Duration::minutes(12)).await;
        // 变体 1、2 在第 2 轮多出一次可能外达的发送。
        if variant != 0 {
            insert_attempt(pool, sale.payment_delivery, 4, at + Duration::minutes(14)).await;
        }
        freeze_current_attempt(pool, sale.payment_delivery, 6, at + Duration::minutes(25)).await;
        if variant == 0 {
            assert_confirmed(&store, &sale, RecoveryConfirmationAction::Reclaimed).await;
        } else {
            assert_rejected(&store, &sale, RecoveryConfirmationAction::Reclaimed).await;
        }
    }

    store.postgres_pool().close().await;
}

/// 没有冻结尝试的发送痕迹同样计数：恢复后外部核对在 Sent 里找到的隔离发送，以及当前旧发送围栏
/// 之外的旧 Sent 绑定，都是另一次可能外达的发送，确认拒绝；当前旧发送围栏自身的旧 Sent 绑定（旧格式
/// Sent 恢复）是同一次发送，不重复计数，照常确认。
#[tokio::test]
async fn postgres_reclaim_confirmation_counts_sends_without_frozen_attempts() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let action = RecoveryConfirmationAction::Reclaimed;

    // 夹具的原付款是没有冻结尝试的旧发送围栏（fence 2、+5 分钟）；另有一次被外部核对隔离的发送。
    let quarantined = voidable_sale(&store, "main", 7_900_401).await;
    let at = created_at(pool, quarantined.recovery_id).await;
    sqlx::query(
        "INSERT INTO public.delivery_external_quarantines
            (scan_id, delivery_id, fencing_version, world_id, external_mail_id, sent_at,
             recipient, subject, collected, observed_at)
         VALUES (gen_random_uuid(), $1, 1, 'main', '7900401', $2, 'Recovery buyer',
                 'Auction equipment delivery', false, $3)",
    )
    .bind(quarantined.payment_delivery)
    .bind(at + Duration::minutes(3))
    .bind(at + Duration::minutes(4))
    .execute(pool)
    .await
    .expect("写入外部核对隔离的发送");
    assert_rejected(&store, &quarantined, action).await;

    // 旧 Sent 绑定落在当前旧发送围栏之外的 fence：另一次已寄达的发送。
    let other_fence = voidable_sale(&store, "main", 7_900_402).await;
    bind_legacy_sent(pool, other_fence.payment_delivery, 1, "7900402").await;
    assert_rejected(&store, &other_fence, action).await;

    // 旧 Sent 绑定就在当前旧发送围栏上：同一次发送，照常确认。
    let same_fence = voidable_sale(&store, "main", 7_900_403).await;
    bind_legacy_sent(pool, same_fence.payment_delivery, 2, "7900403").await;
    assert_confirmed(&store, &same_fence, action).await;

    store.postgres_pool().close().await;
}

/// 写入没有冻结尝试的旧 Sent 绑定（`expectation_fencing_version` 为空）。
async fn bind_legacy_sent(pool: &PgPool, delivery_id: Uuid, fence: i64, mid: &str) {
    sqlx::query(
        "INSERT INTO public.delivery_sent_bindings
            (world_id, external_mail_id, delivery_id, fencing_version)
         VALUES ('main', $1, $2, $3)",
    )
    .bind(mid)
    .bind(delivery_id)
    .bind(fence)
    .execute(pool)
    .await
    .expect("写入旧 Sent 绑定");
}

/// 装备收回：交付在本轮退件之后经重试再次寄出（审计复现的“重试后再次发送结束再确认”）、只在
/// 退件之后留下冻结发送尝试，或退件之前另有一次可能外达的发送，确认都拒绝，退款不激活。
#[tokio::test]
async fn postgres_equipment_recovered_rejects_unattributable_sends() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    for (sequence, case) in [(7_900_101_i64, 0), (7_900_102, 1), (7_900_103, 2)] {
        // 夹具：交付在 +15 分钟寄出，退件在 +40 分钟收到并被消费。
        let recovered = insert_recovered_sale(pool, sequence).await;
        let at = created_at(pool, recovered.sale.recovery_id).await;
        let delivery = recovered.handoff_delivery;
        match case {
            0 => resend_after_return(pool, delivery, at + Duration::minutes(60)).await,
            1 => insert_attempt(pool, delivery, 2, at + Duration::minutes(60)).await,
            _ => insert_attempt(pool, delivery, 2, at + Duration::minutes(12)).await,
        }
        let refund_before: String = sqlx::query_scalar(
            "SELECT business_state FROM public.settlement_obligations WHERE id = $1",
        )
        .bind(recovered.sale.refund)
        .fetch_one(pool)
        .await
        .expect("读取退款义务");
        let handoff_before = fulfillment(pool, recovered.handoff).await;
        let error = store
            .confirm_recovery(confirmation(
                &recovered.sale,
                RecoveryConfirmationAction::EquipmentRecovered,
            ))
            .await
            .expect_err("交付的发送不能唯一归属时不能确认装备收回");
        assert!(
            matches!(
                error,
                ApplicationError::RecoveryCommand(RecoveryCommandError::SendUnattributed)
            ),
            "{case}: {error:?}"
        );
        assert_eq!(
            recovery_state(pool, recovered.sale.recovery_id).await,
            ("reclaiming".to_owned(), None)
        );
        assert_eq!(fulfillment(pool, recovered.handoff).await, handoff_before);
        let refund_after: String = sqlx::query_scalar(
            "SELECT business_state FROM public.settlement_obligations WHERE id = $1",
        )
        .bind(recovered.sale.refund)
        .fetch_one(pool)
        .await
        .expect("读取退款义务");
        assert_eq!(refund_after, refund_before, "买方退款不能被激活");
        assert!(dispositions(pool, delivery).await.is_empty());
        assert_eq!(closure_audits(pool, &recovered.sale).await, 0);
    }

    store.postgres_pool().close().await;
}
