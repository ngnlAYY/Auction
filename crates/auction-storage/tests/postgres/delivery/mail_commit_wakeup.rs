//! 入站邮件批次提交后的投递唤醒：批次决定里含已入队投递时，提交后发送
//! `auction_delivery_ready` 提示；不含投递的批次不发。唤醒只是提示，不改变任何持久状态。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use auction_application::admin::{AdminMailIntakeFilter, AdminMailReader};
use auction_application::delivery::DeliveryWorkStore;
use auction_application::mail::{
    MailPollBatch, MailPollClaim, MailPollCompletion, MailPollIntake, MailPollReadJournal,
    MailPollReadSummary, MailPollStore,
};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use uuid::Uuid;

use crate::support;

const MAILBOX: &str = "wakeup-mailbox";
const MANUAL_KEY_SUBJECT: &str = "补充说明：登录 key 幽默 符号! 手工设置的密码含有各种字符";
const PADDED_MANUAL_KEY_SUBJECT: &str = "补充说明：登录 key  中 abcdefghijklmnopqrstuvwxyz!";
const SHORT_LABELED_KEY_SUBJECT: &str = "补充说明：LoGiN KeY 短 值";
const EQUALS_KEY_SUBJECT: &str = "补充说明：LoGiN KeY =短 值";
const REDACTED_MANUAL_KEY_SUBJECT: &str = "补充说明：登录 key[REDACTED]";
const REDACTED_SHORT_LABELED_KEY_SUBJECT: &str = "补充说明：LoGiN KeY[REDACTED]";
const ATTACHMENT_KEY_SUBJECT: &str = "附件 3D2F1E0C9B";

async fn initialize_mailbox(store: &DatabaseStore, now: Timestamp) {
    store
        .ensure_state(
            MAILBOX.to_owned(),
            now,
            now + auction_time::Duration::minutes(1),
            Duration::from_secs(3600),
        )
        .await
        .expect("初始化邮箱状态");
}

async fn claim(store: &DatabaseStore) -> MailPollClaim {
    let now = Timestamp::now();
    MailPollStore::claim_due(
        store,
        MAILBOX.to_owned(),
        "wakeup-worker".to_owned(),
        now,
        now + auction_time::Duration::minutes(2),
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
    let now = Timestamp::now();
    MailPollBatch {
        mailbox_key: claim.mailbox_key.clone(),
        claim_token: claim.claim_token,
        next_cursor: Some(external_mail_id.to_owned()),
        polled_at: now,
        next_poll_at: now + auction_time::Duration::minutes(1),
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
    let now = Timestamp::now();
    initialize_mailbox(&store, now).await;

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

#[tokio::test]
async fn postgres_mail_subject_redaction_preserves_staged_identity() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let now = Timestamp::now().duration_trunc(auction_time::Duration::seconds(1)).unwrap();
    initialize_mailbox(&store, now).await;

    let colon_key_subject = format!("补充说明：登录 key {}:", " ".repeat(31));
    let mut cursor = None;
    for (mid, raw, safe) in [
        ("9001", "key: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "key: [REDACTED]"),
        (
            "9002",
            "https://example.invalid/equip/123/0123abcdef",
            "https://example.invalid/equip/123/[REDACTED]",
        ),
        ("9003", MANUAL_KEY_SUBJECT, REDACTED_MANUAL_KEY_SUBJECT),
        ("9004", ATTACHMENT_KEY_SUBJECT, "附件 [REDACTED]"),
        ("9005", "拍卖问题", "拍卖问题"),
        ("9006", PADDED_MANUAL_KEY_SUBJECT, REDACTED_MANUAL_KEY_SUBJECT),
        ("9007", SHORT_LABELED_KEY_SUBJECT, REDACTED_SHORT_LABELED_KEY_SUBJECT),
        ("9008", colon_key_subject.as_str(), REDACTED_MANUAL_KEY_SUBJECT),
        ("9009", EQUALS_KEY_SUBJECT, REDACTED_SHORT_LABELED_KEY_SUBJECT),
    ] {
        let claim = claim(&store).await;
        assert_eq!(claim.cursor_value.as_deref(), cursor);
        let summary = MailPollReadSummary {
            external_mail_id: mid.to_owned(),
            world_id: "main".to_owned(),
            sender: "Player".to_owned(),
            subject: raw.to_owned(),
            received_at: now,
        };
        if mid == "9005" {
            // 模拟旧 Worker 已登记的原主题；升级后同一身份照常核验并消费。
            sqlx::query(
                "INSERT INTO public.mail_poll_pending_reads
                     (mailbox_key, world_id, external_mail_id, sender, subject, received_at)
                 VALUES ($1, 'main', $2, 'Player', $3, $4)",
            )
            .bind(MAILBOX)
            .bind(mid)
            .bind(raw)
            .bind(now)
            .execute(store.postgres_pool())
            .await
            .unwrap();
        }
        for _ in 0..2 {
            store
                .stage_reads(claim.clone(), "main".to_owned(), vec![summary.clone()])
                .await
                .unwrap();
        }
        let pending = store.pending_reads(claim.clone(), "main".to_owned(), 1).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].external_mail_id, mid);
        if mid == "9005" {
            assert_eq!(pending[0].subject, raw);
        } else {
            assert!(pending[0].subject.starts_with("sha256:  "));
            assert!(!pending[0].subject.contains(raw));
        }
        let mut conflicting = summary;
        conflicting.subject.push('!');
        let conflict = store
            .stage_reads(claim.clone(), "main".to_owned(), vec![conflicting])
            .await
            .expect_err("重复 MID 的主题冲突须拒绝");
        assert!(conflict.to_string().contains("MoogleMail 待读身份冲突"));

        let mut batch = batch(&claim, mid, serde_json::json!({}));
        let item = &mut batch.items[0];
        item.id = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("auction/inbound-mail-v1/{MAILBOX}/main/{mid}").as_bytes(),
        );
        item.sender = Some("Player".to_owned());
        item.subject = Some(raw.to_owned());
        item.received_at = now;
        let intake_id = item.id;
        let state = store.commit(claim, batch).await.unwrap();
        assert_eq!(state.cursor_value.as_deref(), Some(mid));
        cursor = Some(mid);
        assert_eq!(
            store
                .lookup_completion(MAILBOX.to_owned(), "main".to_owned(), mid.to_owned(), now)
                .await
                .unwrap(),
            MailPollCompletion::Completed,
        );
        let queued: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM public.mail_poll_pending_reads WHERE mailbox_key = $1)",
        )
        .bind(MAILBOX)
        .fetch_one(store.postgres_pool())
        .await
        .unwrap();
        assert!(!queued, "入站与游标提交必须同时清除待读前缀");
        let subject: String =
            sqlx::query_scalar("SELECT subject FROM public.inbound_mail WHERE id = $1")
                .bind(intake_id)
                .fetch_one(store.postgres_pool())
                .await
                .unwrap();
        assert_eq!(subject, safe);
    }
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_admin_mail_reads_scrub_legacy_subjects() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let now = Timestamp::now();
    let colon_key_subject = format!("补充说明：登录 key {}:", " ".repeat(31));
    let cases = [
        ("9011", MANUAL_KEY_SUBJECT, REDACTED_MANUAL_KEY_SUBJECT),
        ("9012", ATTACHMENT_KEY_SUBJECT, "附件 [REDACTED]"),
        ("9013", "拍卖问题", "拍卖问题"),
        ("9014", PADDED_MANUAL_KEY_SUBJECT, REDACTED_MANUAL_KEY_SUBJECT),
        ("9015", SHORT_LABELED_KEY_SUBJECT, REDACTED_SHORT_LABELED_KEY_SUBJECT),
        ("9016", colon_key_subject.as_str(), REDACTED_MANUAL_KEY_SUBJECT),
        ("9017", EQUALS_KEY_SUBJECT, REDACTED_SHORT_LABELED_KEY_SUBJECT),
    ];
    for (mid, raw, _) in cases {
        sqlx::query(
            "INSERT INTO public.inbound_mail
                 (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
                  received_at, status, decision_code, processed_at, sender, subject)
             VALUES ($1, $2, $3, 'main', 'unknown', $4, $5, 'rejected',
                     'unknown_subject', $5, 'Player', $6)",
        )
        .bind(Uuid::new_v4())
        .bind(MAILBOX)
        .bind(mid)
        .bind([1_u8; 32].as_slice())
        .bind(now)
        .bind(raw)
        .execute(store.postgres_pool())
        .await
        .unwrap();
    }
    let page = store
        .list_intakes(AdminMailIntakeFilter {
            mailbox_key: Some(MAILBOX.to_owned()),
            world_id: None,
            kind: None,
            status: None,
            decision: None,
            received_from: None,
            received_to: None,
            page: 1,
            page_size: 10,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), cases.len());
    for (mid, raw, safe) in cases {
        let summary = page.items.iter().find(|item| item.external_mail_id == mid).unwrap();
        assert_eq!(summary.subject.as_deref(), Some(safe));
        let detail = store.intake_detail(summary.id).await.unwrap().unwrap();
        assert_eq!(detail.summary.subject.as_deref(), Some(safe));
        let stored: String =
            sqlx::query_scalar("SELECT subject FROM public.inbound_mail WHERE id = $1")
                .bind(summary.id)
                .fetch_one(store.postgres_pool())
                .await
                .unwrap();
        assert_eq!(stored, raw);
    }
    store.postgres_pool().close().await;
}
