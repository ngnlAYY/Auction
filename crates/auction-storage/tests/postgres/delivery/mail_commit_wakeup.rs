//! 入站邮件批次提交后的投递唤醒：批次决定里含已入队投递时，提交后发送
//! `auction_delivery_ready` 提示；不含投递的批次不发。唤醒只是提示，不改变任何持久状态。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use auction_application::delivery::DeliveryWorkStore;
use auction_application::mail::{MailPollBatch, MailPollClaim, MailPollIntake, MailPollStore};
use auction_storage::DatabaseStore;
use chrono::Utc;
use uuid::Uuid;

use crate::support;

const MAILBOX: &str = "wakeup-mailbox";

async fn claim(store: &DatabaseStore) -> MailPollClaim {
    let now = Utc::now();
    MailPollStore::claim_due(
        store,
        MAILBOX.to_owned(),
        "wakeup-worker".to_owned(),
        now,
        now + chrono::Duration::minutes(2),
        1,
        true,
    )
    .await
    .expect("领取邮箱")
    .pop()
    .expect("强制领取必须返回邮箱租约")
}

fn batch(
    claim: &MailPollClaim,
    external_mail_id: &str,
    decision_metadata: serde_json::Value,
) -> MailPollBatch {
    let now = Utc::now();
    MailPollBatch {
        mailbox_key: claim.mailbox_key.clone(),
        claim_token: claim.claim_token,
        next_cursor: Some(external_mail_id.to_owned()),
        polled_at: now,
        next_poll_at: now + chrono::Duration::minutes(1),
        items: vec![MailPollIntake {
            id: Uuid::new_v4(),
            external_mail_id: external_mail_id.to_owned(),
            sender: None,
            subject: None,
            world_id: "main".to_owned(),
            mail_kind: "unknown".to_owned(),
            status: "rejected".to_owned(),
            content_digest: [7; 32],
            received_at: now,
            decision_code: "unknown_subject".to_owned(),
            decision_metadata,
            message_display: None,
            equipment_diagnostic: None,
            processed_at: now,
        }],
    }
}

#[tokio::test]
async fn postgres_mail_batch_commit_wakes_delivery_only_with_deliveries() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let mut wakeup = store.subscribe_wakeups().await.expect("订阅投递唤醒");
    let now = Utc::now();
    store
        .ensure_state(
            MAILBOX.to_owned(),
            now,
            now + chrono::Duration::minutes(1),
            Duration::from_secs(3600),
        )
        .await
        .expect("初始化邮箱状态");

    let first = claim(&store).await;
    MailPollStore::commit(&store, first.clone(), batch(&first, "9001", serde_json::json!({})))
        .await
        .expect("提交不含投递的批次");
    assert!(
        tokio::time::timeout(Duration::from_millis(500), wakeup.wait()).await.is_err(),
        "不含投递的批次不得发送唤醒"
    );

    let second = claim(&store).await;
    let metadata = serde_json::json!({ "delivery_ids": [Uuid::new_v4()] });
    MailPollStore::commit(&store, second.clone(), batch(&second, "9002", metadata))
        .await
        .expect("提交含投递的批次");
    tokio::time::timeout(Duration::from_secs(5), wakeup.wait())
        .await
        .expect("含投递的批次必须在提交后唤醒投递循环")
        .expect("读取唤醒提示");
    drop(wakeup);
    store.postgres_pool().close().await;
}
