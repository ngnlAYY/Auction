//! 确认端把恢复记录当前的退件与 Take 绑定到被收回的那次发送（PostgreSQL 18）。
//!
//! 旧版本 Admin 投递台可以在恢复流程消费退件之后把原投递改回待发送，同一投递随即在新 fence 上
//! 再次寄出；新发送是另一封邮件，与旧退件和旧 Take 证明无关。收回、取消与装备收回的确认遇到退件
//! 之后的发送（冻结发送尝试或当前发送围栏）一律拒绝，什么都不改；退件之前的历次发送照常放行。
//! 收件箱时间只到分钟，退件收件时刻之后 3 分钟起的发送即使早于消费也算退件之后。

use auction_application::core::ApplicationError;
use auction_application::recovery::{
    RecoveryCommandError, RecoveryCommandRepository, RecoveryConfirmationAction,
};
use auction_storage::DatabaseStore;
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use super::recovered::fixture::insert_recovered_sale;
use super::{closure_audits, confirmation, dispositions, fulfillment};
use crate::support;

/// 夹具里恢复记录的创建时刻；夹具的发送、退件与消费时刻都相对它偏移。
async fn created_at(pool: &PgPool, recovery_id: Uuid) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT created_at FROM public.settlement_recoveries WHERE id = $1")
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取恢复记录创建时刻")
}

/// 给投递追加一条冻结发送尝试（fence 取当前 fence + 1），开始时刻为 `sending_at`；不改投递行。
async fn insert_send_attempt(pool: &PgPool, delivery_id: Uuid, sending_at: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO public.delivery_send_attempts
            (delivery_id, fencing_version, command_id, request_digest, world_id, sending_at,
             expectation)
         SELECT id, fencing_version + 1, gen_random_uuid(), decode(repeat('f1', 32), 'hex'),
                payload_world, $2,
                jsonb_build_object('world_id', payload_world, 'recipient', recipient,
                    'subject', 'Recovery binding', 'body_digest', to_jsonb(array_fill(7, ARRAY[32])),
                    'credits', 0, 'cod', 0, 'equipment_ids', '[]'::jsonb,
                    'credential', 'null'::jsonb)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(delivery_id)
    .bind(sending_at)
    .execute(pool)
    .await
    .expect("写入冻结发送尝试");
}

/// 模拟退件之后经旧 Admin 缺口重试并再次寄出成功：fence 推进、发送围栏落在 `sending_at`，投递
/// 仍是成功，并留下同一 fence 的冻结发送尝试。
async fn resend_after_return(pool: &PgPool, delivery_id: Uuid, sending_at: DateTime<Utc>) {
    insert_send_attempt(pool, delivery_id, sending_at).await;
    sqlx::query(
        "UPDATE public.deliveries
         SET fencing_version = fencing_version + 1, status = 'succeeded',
             sending_at = $2, completed_at = $2, updated_at = $2
         WHERE id = $1",
    )
    .bind(delivery_id)
    .bind(sending_at)
    .execute(pool)
    .await
    .expect("模拟退件之后的再次寄出");
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

async fn recovery_state(pool: &PgPool, recovery_id: Uuid) -> (String, Option<String>) {
    sqlx::query_as("SELECT stage, resolution FROM public.settlement_recoveries WHERE id = $1")
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取恢复记录")
}

/// 确认失败关闭：返回 `NotConfirmable`，恢复记录、付款履约、处置与收口审计都保持原样。
async fn assert_rejected(store: &DatabaseStore, sale: &Sale, action: RecoveryConfirmationAction) {
    let pool = store.postgres_pool();
    let payment_before = fulfillment(pool, sale.payment).await;
    let seller_before = fulfillment(pool, sale.seller).await;
    let error = store
        .confirm_recovery(confirmation(sale, action))
        .await
        .expect_err("退件之后又寄出的投递不能与旧退件拼接确认");
    assert!(
        matches!(error, ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable)),
        "{action:?}: {error:?}"
    );
    assert_eq!(recovery_state(pool, sale.recovery_id).await, ("reclaiming".to_owned(), None));
    assert_eq!(fulfillment(pool, sale.payment).await, payment_before);
    assert_eq!(fulfillment(pool, sale.seller).await, seller_before);
    assert!(dispositions(pool, sale.payment_delivery).await.is_empty());
    assert_eq!(closure_audits(pool, sale).await, 0);
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
/// 收件之后与消费之前，确认都拒绝；退件之前的冻结发送尝试照常放行。
#[tokio::test]
async fn postgres_voiding_confirmation_rejects_sends_after_the_return() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    for (world, base, action) in [
        ("main", 7_900_001_i64, RecoveryConfirmationAction::Reclaimed),
        ("isekai", 7_900_011, RecoveryConfirmationAction::Cancelled),
    ] {
        // 夹具：原付款在 +5 分钟发送，退件在 +30 分钟收到，+31 分钟被恢复流程消费。
        let resent = voidable_sale(&store, world, base).await;
        let at = created_at(pool, resent.recovery_id).await;
        resend_after_return(pool, resent.payment_delivery, at + Duration::minutes(50)).await;
        assert_rejected(&store, &resent, action).await;

        let attempt_only = voidable_sale(&store, world, base + 1).await;
        let at = created_at(pool, attempt_only.recovery_id).await;
        insert_send_attempt(pool, attempt_only.payment_delivery, at + Duration::minutes(40)).await;
        assert_rejected(&store, &attempt_only, action).await;

        let before_consumption = voidable_sale(&store, world, base + 2).await;
        let at = created_at(pool, before_consumption.recovery_id).await;
        move_return_receipt(pool, before_consumption.recovery_id, at + Duration::minutes(20)).await;
        insert_send_attempt(pool, before_consumption.payment_delivery, at + Duration::minutes(25))
            .await;
        assert_rejected(&store, &before_consumption, action).await;

        // 正例：退件之前的冻结发送尝试就是被收回的那次，确认照常收口。
        let returned = voidable_sale(&store, world, base + 3).await;
        let at = created_at(pool, returned.recovery_id).await;
        insert_send_attempt(pool, returned.payment_delivery, at + Duration::minutes(5)).await;
        store
            .confirm_recovery(confirmation(&returned, action))
            .await
            .expect("退件之前的发送照常确认");
        assert_eq!(
            recovery_state(pool, returned.recovery_id).await.1.as_deref(),
            Some(action.as_str())
        );
        assert_eq!(dispositions(pool, returned.payment_delivery).await.len(), 1);
    }

    store.postgres_pool().close().await;
}

/// 装备收回：交付在本轮退件消费之后经重试再次寄出并取得新的可信 Sent 证明（审计复现的“重试后
/// 再次发送结束再确认”），或只在退件之后留下冻结发送尝试，确认都拒绝，退款不激活。
#[tokio::test]
async fn postgres_equipment_recovered_rejects_sends_after_the_return() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    for (sequence, attempt_only) in [(7_900_101_i64, false), (7_900_102, true)] {
        // 夹具：交付在 +15 分钟寄出，退件在 +40 分钟收到并被消费。
        let recovered = insert_recovered_sale(pool, sequence).await;
        let at = created_at(pool, recovered.sale.recovery_id).await;
        if attempt_only {
            insert_send_attempt(pool, recovered.handoff_delivery, at + Duration::minutes(60)).await;
        } else {
            resend_after_return(pool, recovered.handoff_delivery, at + Duration::minutes(60)).await;
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
            .expect_err("退件之后又寄出的交付不能确认装备收回");
        assert!(
            matches!(
                error,
                ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable)
            ),
            "{error:?}"
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
        assert!(dispositions(pool, recovered.handoff_delivery).await.is_empty());
        assert_eq!(closure_audits(pool, &recovered.sale).await, 0);
    }

    store.postgres_pool().close().await;
}
