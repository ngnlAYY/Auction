//! 确认已处置只追加 operator_acknowledged 事实，并在 Admin 详情中可见。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[allow(dead_code)]
use crate::support;

use auction_application::delivery::{
    DeliveryAcknowledgementCommand, DeliveryAdminPort, DeliveryDispositionCommandError,
    DeliveryDispositionPort,
};
use auction_domain::DeliveryDispositionActorSource;
use chrono::Utc;
use uuid::Uuid;

/// 建立一条无资金/资产影响的已失败投递；payload 不携带 credits/cod/附件。
async fn insert_failed_notification(pool: &sqlx::PgPool) -> (Uuid, i64) {
    let id = Uuid::new_v4();
    let fencing_version = 1_i64;
    let created_at = Utc::now() - chrono::Duration::minutes(3);
    let claimed_at = created_at + chrono::Duration::minutes(1);
    let completed_at = claimed_at + chrono::Duration::minutes(1);
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             created_at, updated_at, failure_code, result_metadata)
         VALUES ($1, $2, decode(repeat('22', 32), 'hex'), 'mail_intake', $3, $4,
                 $5::jsonb, 'failed', $6, 1, $7, $7, $8, $9, $10,
                 'verified_not_sent', '{}'::jsonb)",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(format!("w2-5:{id}"))
    .bind(format!("player-{id}"))
    .bind(
        r#"{"schema_version":1,"operation":"send","template":"register_key",
            "credits":0,"cod":0,"attachment_ids":[]}"#,
    )
    .bind(fencing_version)
    .bind(claimed_at)
    .bind(completed_at)
    .bind(created_at)
    .bind(completed_at)
    .execute(pool)
    .await
    .expect("写入失败投递");
    (id, fencing_version)
}

#[tokio::test]
async fn postgres_acknowledge_appends_disposition_and_is_visible_in_detail() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let (delivery_id, fencing_version) = insert_failed_notification(store.postgres_pool()).await;
    let command_id = Uuid::new_v4();
    let reason = "historical failure handled outside the outbox";
    let command = DeliveryAcknowledgementCommand {
        command_id,
        delivery_id,
        expected_fencing_version: fencing_version,
        actor_source: DeliveryDispositionActorSource::Admin,
        actor_id: "admin".to_owned(),
        reason: reason.to_owned(),
        operator_identity: None,
        request_id: None,
        request_hash: DeliveryAcknowledgementCommand::request_hash(
            delivery_id,
            fencing_version,
            "admin",
            reason,
        ),
    };

    let result = store.acknowledge(command.clone()).await.expect("确认处置");
    assert_eq!(result.kind, "operator_acknowledged");
    assert_eq!(result.fencing_version, fencing_version);

    let facts = store.dispositions(delivery_id).await.expect("读取处置事实");
    let fact = facts.iter().find(|fact| fact.kind == "operator_acknowledged").expect("处置事实");
    assert_eq!(fact.reason.as_deref(), Some(reason));

    // 统一处置表中的预检退役也要出现在详情中；它没有命令账本身份，
    // 详情以处置事实自身 UUID 作为稳定展示身份。
    sqlx::query(
        "INSERT INTO public.delivery_dispositions
            (delivery_id, fencing_version, kind, actor_source, actor_id,
             reason_code, resolves_alert, detail)
         VALUES ($1, $2, 'preflight_retired', 'worker', 'delivery-worker',
                 'lot_watch_invalidated', true, '{}'::jsonb)",
    )
    .bind(delivery_id)
    .bind(fencing_version)
    .execute(store.postgres_pool())
    .await
    .expect("写入预检退役处置");

    let detail = store.detail(delivery_id).await.expect("读取 Admin 详情");
    assert!(
        detail.dispositions.iter().any(|value| value.kind == "operator_acknowledged"),
        "Admin 详情必须回显确认已处置事实"
    );
    assert!(
        detail.dispositions.iter().any(|value| value.kind == "preflight_retired"),
        "Admin 详情必须回显统一处置表中的预检退役"
    );

    // 同一命令同参重放返回原结果；不同原因同命令 ID 必须冲突。
    let replay = store.acknowledge(command).await.expect("同参重放");
    assert_eq!(replay.kind, "operator_acknowledged");
    let conflicting = DeliveryAcknowledgementCommand {
        command_id,
        delivery_id,
        expected_fencing_version: fencing_version,
        actor_source: DeliveryDispositionActorSource::Admin,
        actor_id: "admin".to_owned(),
        reason: "different reason".to_owned(),
        operator_identity: None,
        request_id: None,
        request_hash: DeliveryAcknowledgementCommand::request_hash(
            delivery_id,
            fencing_version,
            "admin",
            "different reason",
        ),
    };
    assert!(matches!(
        store.acknowledge(conflicting).await,
        Err(auction_application::core::ApplicationError::DeliveryDisposition(
            DeliveryDispositionCommandError::CommandConflict
        ))
    ));
}

#[tokio::test]
async fn postgres_acknowledge_rejects_funded_or_obligation_delivery() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let (delivery_id, fencing_version) = insert_failed_notification(store.postgres_pool()).await;
    // 追加非零 Credits 后不再具备确认资格；服务端必须从数据库重新读取，不信命令形状。
    sqlx::query(
        "UPDATE public.deliveries
         SET payload = payload || '{\"credits\":1}'::jsonb
         WHERE id = $1",
    )
    .bind(delivery_id)
    .execute(store.postgres_pool())
    .await
    .expect("修改测试载荷");
    let reason = "must not be accepted";
    let command = DeliveryAcknowledgementCommand {
        command_id: Uuid::new_v4(),
        delivery_id,
        expected_fencing_version: fencing_version,
        actor_source: DeliveryDispositionActorSource::Admin,
        actor_id: "admin".to_owned(),
        reason: reason.to_owned(),
        operator_identity: None,
        request_id: None,
        request_hash: DeliveryAcknowledgementCommand::request_hash(
            delivery_id,
            fencing_version,
            "admin",
            reason,
        ),
    };
    assert!(matches!(
        store.acknowledge(command).await,
        Err(auction_application::core::ApplicationError::DeliveryDisposition(
            DeliveryDispositionCommandError::NotAcknowledgeable
        ))
    ));
}
