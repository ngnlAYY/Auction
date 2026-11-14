//! 出站邮件分类开关在投递领取侧的 SQL 行为（runtime `operations.mail.send_classes`）。
//!
//! 关闭某类时，匹配的 pending 投递必须保留在 outbox 且不被领取；重新开启后按正常
//! 调度补发。只验证领取过滤，不发送任何外部请求。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::delivery::{DeliveryClaimExclusions, DeliveryWorkStore};
use auction_application::mail::MailSendClasses;
use chrono::{Duration, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::support;

const WORKER: &str = "delivery-send-classes-worker";

/// 插入一条待发送投递；`origin`/`template` 决定它所属的发送分类。
async fn insert_delivery(connection: &mut PgConnection, origin: &str, template: &str) -> Uuid {
    let id = Uuid::new_v4();
    let payload = serde_json::json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "external_mail_id": id.to_string(),
        "template": template,
        "result_code": "test",
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
        "body": "Frozen body",
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, decode(repeat('22', 32), 'hex'), $3, $4, $5, $6::jsonb, 'pending')",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(origin)
    .bind(format!("send-classes:{id}"))
    .bind(format!("player-{id}"))
    .bind(payload.to_string())
    .execute(&mut *connection)
    .await
    .expect("写入待发送投递");
    id
}

async fn claim(store: &auction_storage::DatabaseStore, classes: MailSendClasses) -> Vec<Uuid> {
    let now = Utc::now();
    let sending_grace = std::time::Duration::from_secs(60);
    store.begin_claim_cycle(now, sending_grace).await.expect("执行领取前清理");
    store
        .claim_due(
            WORKER.to_owned(),
            now,
            now + Duration::minutes(2),
            sending_grace,
            DeliveryClaimExclusions {
                suppressed_classes: classes,
                ..DeliveryClaimExclusions::default()
            },
            10,
        )
        .await
        .expect("领取投递")
        .into_iter()
        .map(|claim| claim.delivery_id)
        .collect()
}

async fn status(connection: &mut PgConnection, id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *connection)
        .await
        .expect("读取投递状态")
}

#[tokio::test]
async fn postgres_closed_send_class_stays_pending_until_reopened() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let mut connection = store.postgres_pool().acquire().await.expect("获取测试连接");

    let summary =
        insert_delivery(&mut connection, "seller_summary", "auction_seller_summary").await;
    let settlement =
        insert_delivery(&mut connection, "settlement", "settlement_seller_payout").await;

    // 关闭玩家汇总类：只领取结算通知，汇总仍为 pending。
    let closed_summary = MailSendClasses { player_summary: false, ..MailSendClasses::default() };
    let claimed = claim(&store, closed_summary).await;
    assert_eq!(claimed, vec![settlement], "关闭类不得被领取");
    assert_eq!(status(&mut connection, summary).await, "pending");
    assert_eq!(status(&mut connection, settlement).await, "claimed");

    // 关闭结算通知类：付款提醒保留在 outbox；COD 与退款等履约动作邮件不受影响。
    let closed_settlement = MailSendClasses {
        player_summary: false,
        settlement_notice: false,
        ..MailSendClasses::default()
    };
    let reminder =
        insert_delivery(&mut connection, "settlement_recovery_action", "recovery_payment_reminder")
            .await;
    let refund = insert_delivery(&mut connection, "settlement", "settlement_buyer_refund").await;
    assert_eq!(claim(&store, closed_settlement).await, vec![refund], "履约动作邮件不得被暂停");
    assert_eq!(status(&mut connection, reminder).await, "pending");

    // 全部重新开启：两条被暂停的投递按原调度补发。
    let reopened = claim(&store, MailSendClasses::default()).await;
    assert_eq!(reopened.len(), 2, "重新开启后应补发两条暂停投递：{reopened:?}");
    assert!(reopened.contains(&summary));
    assert!(reopened.contains(&reminder));
}
