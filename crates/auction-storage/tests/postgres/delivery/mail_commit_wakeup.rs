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
use auction_application::settlement::SYSTEM_RETURN_SENDER;
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

#[tokio::test]
async fn postgres_mail_poll_renewal_rejects_a_lease_expired_by_database_time() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    initialize_mailbox(&store, Timestamp::now()).await;
    let original = claim(&store).await;
    let renewed_at = Timestamp::now();
    let lease_until = original.lease_until + auction_time::Duration::minutes(1);
    let renewed = MailPollStore::renew(&store, original, renewed_at, lease_until)
        .await
        .expect("有效租约可以续期");
    assert_eq!(renewed.lease_until, lease_until);

    let expired_at = renewed_at - auction_time::Duration::minutes(1);
    sqlx::query(
        "UPDATE public.mail_poll_state SET claim_expires_at = $2
         WHERE mailbox_key = $1",
    )
    .bind(MAILBOX)
    .bind(expired_at)
    .execute(pool)
    .await
    .expect("使轮询租约过期");
    assert!(
        MailPollStore::renew(
            &store,
            renewed,
            expired_at - auction_time::Duration::seconds(1),
            lease_until + auction_time::Duration::minutes(1)
        )
        .await
        .is_err(),
        "调用方时刻早于租约截止，也不能续期在数据库时钟下已过期的租约"
    );
    let matches: bool = sqlx::query_scalar(
        "SELECT (claim_expires_at = $2 AND updated_at = $3) IS TRUE
         FROM public.mail_poll_state WHERE mailbox_key = $1",
    )
    .bind(MAILBOX)
    .bind(expired_at)
    .bind(renewed_at)
    .fetch_one(pool)
    .await
    .expect("核对续租与拒绝后的时间");
    assert!(matches, "续期使用调用方时间，拒绝的续期不得改写租约");
    pool.close().await;
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
    let returned_subject = "[RTS] Auction payment request [5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a]";
    let padded_returned_subject = format!("{returned_subject} ");
    let long_text = "x".repeat(491);
    let long_subject = format!("          {long_text} login key ");
    let safe_long_subject = format!("{long_text} login key[REDACTED]");
    let player = ("Player", "Player");
    let system = (SYSTEM_RETURN_SENDER, SYSTEM_RETURN_SENDER);
    let cases = [
        ("9011", player, MANUAL_KEY_SUBJECT, REDACTED_MANUAL_KEY_SUBJECT),
        ("9012", player, ATTACHMENT_KEY_SUBJECT, "附件 [REDACTED]"),
        ("9013", player, "拍卖问题", "拍卖问题"),
        ("9014", player, PADDED_MANUAL_KEY_SUBJECT, REDACTED_MANUAL_KEY_SUBJECT),
        ("9015", player, SHORT_LABELED_KEY_SUBJECT, REDACTED_SHORT_LABELED_KEY_SUBJECT),
        ("9016", player, colon_key_subject.as_str(), REDACTED_MANUAL_KEY_SUBJECT),
        ("9017", player, EQUALS_KEY_SUBJECT, REDACTED_SHORT_LABELED_KEY_SUBJECT),
        ("9018", ("Player ", "Player"), "拍卖问题 \u{3000}", "拍卖问题"),
        ("9019", player, "补充说明：登录 key   ", REDACTED_MANUAL_KEY_SUBJECT),
        ("9020", ("\t\u{3000}", "[REDACTED]"), "\t\u{3000}", "[REDACTED]"),
        ("9021", player, "多行\n主题", "[REDACTED]"),
        ("9022", system, returned_subject, returned_subject),
        ("9023", system, padded_returned_subject.as_str(), "[REDACTED]"),
        ("9024", ("MoogleMail ", "[REDACTED]"), returned_subject, returned_subject),
        ("9025", player, long_subject.as_str(), safe_long_subject.as_str()),
    ];
    for (mid, (sender, _), raw, _) in cases {
        sqlx::query(
            "INSERT INTO public.inbound_mail
                 (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
                  received_at, status, decision_code, processed_at, sender, subject)
             VALUES ($1, $2, $3, 'main', 'unknown', $4, $5, 'rejected',
                     'unknown_subject', $5, $6, $7)",
        )
        .bind(Uuid::new_v4())
        .bind(MAILBOX)
        .bind(mid)
        .bind([1_u8; 32].as_slice())
        .bind(now)
        .bind(sender)
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
            page_size: 20,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), cases.len());
    assert_eq!(page.total, i64::try_from(cases.len()).unwrap());
    for (mid, (sender, safe_sender), raw, safe) in cases {
        let summary = page.items.iter().find(|item| item.external_mail_id == mid).unwrap();
        assert_eq!(summary.sender.as_deref(), Some(safe_sender));
        assert_eq!(summary.subject.as_deref(), Some(safe));
        let detail = store.intake_detail(summary.id).await.unwrap().unwrap();
        assert_eq!(detail.summary.sender.as_deref(), Some(safe_sender));
        assert_eq!(detail.summary.subject.as_deref(), Some(safe));
        let (stored_sender, stored_subject, content_digest): (String, String, Vec<u8>) =
            sqlx::query_as(
                "SELECT sender, subject, content_digest FROM public.inbound_mail WHERE id = $1",
            )
            .bind(summary.id)
            .fetch_one(store.postgres_pool())
            .await
            .unwrap();
        assert_eq!(stored_sender, sender);
        assert_eq!(stored_subject, raw);
        assert_eq!(content_digest, [1; 32]);
    }
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_admin_mail_reads_replace_invalid_legacy_display_fields() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let mailbox = "m".repeat(257);
    let mid = "9001\t";
    let world = "legacy\tworld";
    let decision = "d".repeat(257);
    let intake_id = Uuid::new_v4();
    let maintenance = database.maintenance_store().await.expect("以维护账号写入历史世界夹具");
    sqlx::query("INSERT INTO public.worlds (id) VALUES ($1)")
        .bind(world)
        .execute(maintenance.postgres_pool())
        .await
        .unwrap();
    maintenance.postgres_pool().close().await;
    // btrim 约束只处理 ASCII 空格，原始标识仍用于数据库筛选和关联。
    sqlx::query(
        "INSERT INTO public.inbound_mail
             (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
              received_at, status, decision_code, processed_at)
         VALUES ($1, $2, $3, $4, 'unknown', $5, $6, 'rejected', $7, $6)",
    )
    .bind(intake_id)
    .bind(&mailbox)
    .bind(mid)
    .bind(world)
    .bind([1_u8; 32].as_slice())
    .bind(Timestamp::now())
    .bind(&decision)
    .execute(pool)
    .await
    .unwrap();
    let page = store
        .list_intakes(AdminMailIntakeFilter {
            mailbox_key: None,
            world_id: None,
            kind: Some("unknown".to_owned()),
            status: Some("rejected".to_owned()),
            decision: None,
            received_from: None,
            received_to: None,
            page: 1,
            page_size: 20,
        })
        .await
        .unwrap();
    let summary = page.items.iter().find(|item| item.id == intake_id).unwrap();
    assert_eq!(summary.mailbox_key, "[REDACTED]");
    assert_eq!(summary.external_mail_id, "[REDACTED]");
    assert_eq!(summary.world_id, "[REDACTED]");
    assert_eq!(summary.decision_code, "[REDACTED]");
    assert_eq!(summary.kind, "unknown");
    assert_eq!(summary.status, "rejected");
    assert_eq!(summary.sender, None);
    assert_eq!(summary.subject, None);
    let detail = store.intake_detail(intake_id).await.unwrap().unwrap();
    assert_eq!(&detail.summary, summary);
    let stored: (String, String, String, String) = sqlx::query_as(
        "SELECT mailbox_key, external_mail_id, world_id, decision_code
         FROM public.inbound_mail WHERE id = $1",
    )
    .bind(intake_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(stored, (mailbox, mid.to_owned(), world.to_owned(), decision));
    pool.close().await;
}
