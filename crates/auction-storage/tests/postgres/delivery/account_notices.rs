//! 账号回信（rekey）经真实存储入队校验与回信预算写入 outbox 的 PostgreSQL 18 用例。
//!
//! 入队校验拒绝合法回信时，邮件处理器只能整封重试，游标停在该信之前，同一 world 的入站
//! 邮箱随之停摆；因此必须经 `DatabaseStore` 的真实入队路径证明 rekey 回信可以落库。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::assert_matches;
use std::sync::Arc;

use auction_application::core::{ApplicationError, RequestHash};
use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminGuardBlocker,
    DeliveryAdminPort, DeliveryFenceOutcome, DeliveryPreparedExpectation, DeliverySendExpectation,
    DeliveryTransportError, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::mail::{
    MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryOrigin,
    MailLoginKeyEnvelope, MailNoticeEnqueue, MailNoticeLimit, MailNotificationTemplate,
    MailPollBatch, MailPollIntake, MailPollStore, MailUserRebindCommand, MailUserRekeyCommand,
    MailUserRekeyWrite, MailUserResultCode, MailUserStore,
};
use auction_domain::CommandId;
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use super::fence::sent_result;
use crate::support;

fn rekey_command(
    template: MailNotificationTemplate,
    result_code: &str,
    user_key_user_id: Option<Uuid>,
) -> MailDeliveryCommand {
    MailDeliveryCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        request_hash: RequestHash::new([5; 32]),
        origin: MailDeliveryOrigin::MailIntake,
        origin_id: "main:4242:send".to_owned(),
        operation: MailDeliveryOperation::Send,
        world_id: "main".to_owned(),
        external_mail_id: "4242".to_owned(),
        recipient: "RekeyPlayer".to_owned(),
        template: Some(template),
        result_code: Some(result_code.to_owned()),
        user_key_user_id,
        credits: 0,
        cod: 0,
        attachment_ids: Vec::new(),
        notification_body: None,
    }
}

fn rekey_limit() -> MailNoticeLimit {
    MailNoticeLimit {
        templates: &["rekey_key", "rekey_result"],
        since: Timestamp::now() - Duration::hours(24),
        maximum: 3,
    }
}

/// 回归：rekey_key 回信带用户 key 引用，必须经入队校验写成 account 类 pending 投递；
/// 同一命令重放返回原 delivery（不变量 1）。
#[tokio::test]
async fn postgres_rekey_key_notice_enqueues_with_user_key_reference() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let user_id = Uuid::new_v4();
    let command = rekey_command(MailNotificationTemplate::RekeyKey, "rekeyed", Some(user_id));

    let outcome =
        store.enqueue_within_limit(command.clone(), rekey_limit()).await.expect("rekey 回信入队");
    let MailNoticeEnqueue::Enqueued(result) = outcome else {
        panic!("预算内的 rekey 回信不得被抑制");
    };
    let (status, payload_user, template, category): (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT status, payload ->> 'user_key_user_id', payload_template,
                public.delivery_send_category(origin_kind, payload)
         FROM public.deliveries WHERE id = $1",
    )
    .bind(result.delivery_id)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取 rekey 投递");
    assert_eq!(status, "pending");
    assert_eq!(payload_user, Some(user_id.to_string()));
    assert_eq!(template.as_deref(), Some("rekey_key"));
    assert_eq!(category.as_deref(), Some("account"));

    let replay = store.enqueue_within_limit(command, rekey_limit()).await.expect("同命令重放");
    assert_eq!(replay, MailNoticeEnqueue::Enqueued(result));

    // 反例：rekey_result 不得携带用户 key 引用，入队前即失败关闭且不写行。
    let invalid =
        rekey_command(MailNotificationTemplate::RekeyResult, "not_registered", Some(user_id));
    let invalid_command_id = invalid.command_id;
    assert!(store.enqueue_within_limit(invalid, rekey_limit()).await.is_err());
    let written: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM public.deliveries WHERE command_id = $1)")
            .bind(invalid_command_id.as_uuid())
            .fetch_one(store.postgres_pool())
            .await
            .expect("读取被拒命令");
    assert!(!written, "被拒的 rekey_result 不得写入 outbox");
    store.postgres_pool().close().await;
}

async fn insert_reply_user(store: &DatabaseStore, mirror: Option<&str>) -> Uuid {
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.users (id, username, game_player_id, public_key_hash)
         VALUES ($1, 'RekeyPlayer', $2, decode(repeat('38', 32), 'hex'))",
    )
    .bind(user_id)
    .bind(mirror)
    .execute(store.postgres_pool())
    .await
    .unwrap();
    user_id
}

async fn register_reply(store: &DatabaseStore) -> (Uuid, DeliveryWorkClaim) {
    let user_id = insert_reply_user(store, Some("RekeyPlayer")).await;
    let claim = claim_login_key_reply(store, user_id, MailNotificationTemplate::RegisterKey).await;
    (user_id, claim)
}

async fn claim_login_key_reply(
    store: &DatabaseStore,
    user_id: Uuid,
    template: MailNotificationTemplate,
) -> DeliveryWorkClaim {
    let (result_code, subject, mail_kind, decision_code) = match template {
        MailNotificationTemplate::RegisterKey => {
            ("registered", "register", "registration", "registration_accepted")
        }
        MailNotificationTemplate::RekeyKey => ("rekeyed", "rekey", "rekey", "rekey_accepted"),
        _ => panic!("夹具只处理带登录 key 的回信"),
    };
    let delivery =
        store.enqueue(rekey_command(template, result_code, Some(user_id))).await.unwrap();
    // 单独入队不具备发送授权，领取必须看到已提交的入站邮件与匹配的命令账本。
    assert!(claim_replies(store).await.is_empty());
    let now = Timestamp::now();
    let mailbox = "login-key-reply";
    let external_mail_id = "4242";
    store
        .ensure_state(
            mailbox.to_owned(),
            now,
            now + Duration::minutes(1),
            std::time::Duration::from_secs(3600),
        )
        .await
        .unwrap();
    let poll = MailPollStore::claim_due(
        store,
        mailbox.to_owned(),
        "fixture-poller".to_owned(),
        now,
        now + Duration::minutes(2),
        1,
        true,
    )
    .await
    .unwrap()
    .pop()
    .expect("领取测试邮箱");
    let polled_at = Timestamp::now();
    MailPollStore::commit(
        store,
        poll.clone(),
        MailPollBatch {
            mailbox_key: mailbox.to_owned(),
            claim_token: poll.claim_token,
            next_cursor: Some(external_mail_id.to_owned()),
            polled_at,
            next_poll_at: polled_at + Duration::minutes(1),
            items: vec![MailPollIntake {
                id: Uuid::new_v5(
                    &Uuid::NAMESPACE_URL,
                    format!("auction/inbound-mail-v1/{mailbox}/main/{external_mail_id}").as_bytes(),
                ),
                external_mail_id: external_mail_id.to_owned(),
                sender: Some("RekeyPlayer".to_owned()),
                subject: Some(subject.to_owned()),
                world_id: "main".to_owned(),
                mail_kind: mail_kind.to_owned(),
                status: "accepted".to_owned(),
                content_digest: [0x55; 32],
                received_at: now,
                decision_code: decision_code.to_owned(),
                decision_metadata: serde_json::json!({"delivery_ids": [delivery.delivery_id]}),
                message_display: None,
                equipment_diagnostic: None,
                processed_at: polled_at,
            }],
        },
    )
    .await
    .expect("提交入站决定和匹配的邮件命令账本");
    let claim = claim_replies(store).await.pop().expect("领取已授权且绑定一致的登录 key 回信");
    assert_eq!(claim.delivery_id, delivery.delivery_id);
    claim
}

async fn claim_replies(store: &DatabaseStore) -> Vec<DeliveryWorkClaim> {
    let now = Timestamp::now();
    let grace = std::time::Duration::from_secs(60);
    store.begin_claim_cycle(now, grace).await.unwrap();
    DeliveryWorkStore::claim_due(
        store,
        "test-worker".to_owned(),
        now,
        now + Duration::minutes(2),
        grace,
        Default::default(),
        1,
    )
    .await
    .unwrap()
}

async fn rename(store: &DatabaseStore, user_id: Uuid) {
    let command_id = CommandId::new();
    let result = store
        .rebind_username(MailUserRebindCommand {
            command_id,
            request_hash: RequestHash::new([6; 32]),
            old_key_hash: [0x38; 32],
            next_username: "NewPlayer".to_owned(),
            history_id: command_id.as_uuid(),
            now: Timestamp::now(),
        })
        .await
        .expect("经账户 owner 换绑原收件账号");
    assert_eq!(result.user_id, Some(user_id));
    assert_eq!(result.code, MailUserResultCode::Rebound);
}

fn expectation(claim: &DeliveryWorkClaim) -> DeliveryPreparedExpectation {
    DeliveryPreparedExpectation::Send(Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: "Fixture".to_owned(),
        body_digest: [0x44; 32],
        credits: 0,
        cod: 0,
        equipment_ids: Vec::new(),
        credential: None,
    }))
}

/// 通过准备前和已提交 sending 后的续租，再提交传输替身的精确 Sent 证明。
async fn assert_key_reply_can_send(store: &DatabaseStore, claim: DeliveryWorkClaim) {
    let now = Timestamp::now();
    DeliveryWorkStore::renew(store, claim.clone(), now, now + Duration::minutes(2))
        .await
        .expect("角色绑定正确，允许准备回信");
    let prepared = expectation(&claim);
    let outcome = store
        .start_sending_authorized(claim.clone(), Timestamp::now(), None, Some(&prepared))
        .await
        .unwrap();
    assert_matches!(outcome, DeliveryFenceOutcome::Sending);
    let now = Timestamp::now();
    DeliveryWorkStore::renew(store, claim.clone(), now, now + Duration::minutes(2))
        .await
        .expect("角色绑定正确，允许开始发送");
    let DeliveryPreparedExpectation::Send(expectation) = prepared else {
        panic!("登录 key 回信必须使用 Send")
    };
    let proof = sent_result(&claim, &expectation, "9090");
    store.succeed(claim.clone(), proof, Timestamp::now()).await.unwrap();
    assert_eq!(store.detail(claim.delivery_id).await.unwrap().delivery.status, "succeeded");
}

#[tokio::test]
async fn postgres_rekey_login_key_reply_sends_for_legacy_account_without_mirror() {
    let database = support::TestDatabase::create().await.unwrap();
    let store = database.store().await.unwrap();
    let user_id = insert_reply_user(&store, None).await;
    let result = store
        .rekey_login_key(MailUserRekeyWrite {
            command: MailUserRekeyCommand {
                command_id: CommandId::new(),
                request_hash: RequestHash::new([7; 32]),
                username: "RekeyPlayer".to_owned(),
                now: Timestamp::now(),
            },
            user_id,
            public_key_hash: [0x54; 32],
            envelope: MailLoginKeyEnvelope::new(
                vec![1; 16],
                vec![2; 12],
                "test-key".to_owned(),
                "test-fingerprint".to_owned(),
            )
            .unwrap(),
        })
        .await
        .expect("旧账号按 username 轮换登录 key");
    assert_eq!(result.code, MailUserResultCode::Rekeyed);
    assert_eq!(result.user_id, Some(user_id));
    let claim = claim_login_key_reply(&store, user_id, MailNotificationTemplate::RekeyKey).await;
    assert_key_reply_can_send(&store, claim).await;
}

#[tokio::test]
async fn postgres_login_key_reply_sends_when_legacy_mirror_disagrees_with_username() {
    let database = support::TestDatabase::create().await.unwrap();
    // 只在本用例的独立克隆库构造迁移前不同名的历史形态；被测端口仍使用运行账号。
    let maintenance = database.maintenance_store().await.unwrap();
    sqlx::query("ALTER TABLE public.users DROP CONSTRAINT users_name_compatibility_check")
        .execute(maintenance.postgres_pool())
        .await
        .unwrap();
    maintenance.postgres_pool().close().await;
    let store = database.store().await.unwrap();
    let user_id = insert_reply_user(&store, Some("HistoricalMirror")).await;
    let claim = claim_login_key_reply(&store, user_id, MailNotificationTemplate::RegisterKey).await;
    assert_key_reply_can_send(&store, claim).await;
}

async fn assert_compatible_failure(
    store: &DatabaseStore,
    claim: &DeliveryWorkClaim,
    sending: bool,
) {
    let (detail, guard, acknowledge) = store.detail_with_guard(claim.delivery_id).await.unwrap();
    assert_eq!(detail.delivery.status, "failed");
    assert!(detail.delivery.retirement_reason.is_none());
    assert!(detail.delivery.reason_code.is_none());
    assert_eq!(detail.delivery.fencing_version, claim.fencing_version);
    assert_eq!(detail.delivery.sending_at.is_some(), sending);
    assert_eq!(detail.attempts.len(), usize::from(sending));
    assert_eq!(
        detail.delivery.failure_code.as_deref(),
        Some(if sending { "preflight_blocked" } else { "invalid_payload" })
    );
    assert_eq!(acknowledge, None, "确定失败可以用原路径确认已处置");
    assert_eq!(
        guard.projected_blocker(DeliveryAdminAction::RetryFailed),
        Some(DeliveryAdminGuardBlocker::RecipientRenamed)
    );
    let metadata: serde_json::Value =
        sqlx::query_scalar("SELECT result_metadata FROM public.deliveries WHERE id = $1")
            .bind(claim.delivery_id)
            .fetch_one(store.postgres_pool())
            .await
            .unwrap();
    assert_eq!(metadata["preflight_reason"], "recipient_renamed");
    assert_eq!(metadata["write_started"], false);
    assert_eq!(metadata["outcome_unknown"], false);
    assert!(metadata.get("reason_code").is_none());
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE entity_type = 'delivery' AND entity_id = $1 AND action = 'delivery.fail'
           AND metadata ->> 'preflight_reason' = 'recipient_renamed'
           AND metadata ->> 'outcome_unknown' = 'false'",
    )
    .bind(claim.delivery_id.to_string())
    .fetch_one(store.postgres_pool())
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
    if !sending {
        // v0.1.8: crates/auction-storage/src/database/delivery/admin/mapping/evidence.rs
        // 的 invalid_payload 分支只接受这些确定未写入的旧字段。
        assert_eq!(metadata["stage"], "preflight_blocked");
        assert_eq!(metadata["failure_code"], "invalid_payload");
        assert!(metadata.get("proof").is_none());
    }
}

async fn assert_binding_rejected(store: &DatabaseStore, claim: &DeliveryWorkClaim, status: &str) {
    let now = Timestamp::now();
    let error = DeliveryWorkStore::renew(store, claim.clone(), now, now + Duration::minutes(2))
        .await
        .unwrap_err();
    assert_matches!(error, ApplicationError::DeliveryAdmin(DeliveryAdminError::RecipientRenamed));
    let current = store.detail(claim.delivery_id).await.unwrap();
    assert_eq!(current.delivery.status, status, "续租只返回拒绝，终态由 Worker 结果路径写入");
    assert_eq!(current.delivery.fencing_version, claim.fencing_version);
    DeliveryWorkStore::fail(
        store,
        claim.clone(),
        DeliveryTransportError {
            failure_code: "preflight_blocked".to_owned(),
            safe_detail: "登录 key 回信的收件角色已换绑".to_owned(),
            outcome_unknown: false,
            external_reference: None,
            result_metadata: serde_json::json!({
                "stage": "preflight_blocked", "preflight_reason": "recipient_renamed", "write_started": false
            }),
        },
        Timestamp::now(),
    ).await.unwrap();
    assert_compatible_failure(store, claim, status == "sending").await;
}

#[tokio::test]
async fn postgres_login_key_reply_fails_before_body_preparation_after_rebind() {
    let database = support::TestDatabase::create().await.unwrap();
    let store = database.store().await.unwrap();
    let (user_id, claim) = register_reply(&store).await;
    let now = Timestamp::now();
    DeliveryWorkStore::renew(&store, claim.clone(), now, now + Duration::minutes(1)).await.unwrap();

    rename(&store, user_id).await;
    // 即使另一账号已使用旧角色名，也不能接收原账号的登录 key。
    sqlx::query(
        "INSERT INTO public.users (id, username, game_player_id, public_key_hash)
                 VALUES ($1, 'RekeyPlayer', 'RekeyPlayer', decode(repeat('39', 32), 'hex'))",
    )
    .bind(Uuid::new_v4())
    .execute(store.postgres_pool())
    .await
    .unwrap();
    assert_binding_rejected(&store, &claim, "claimed").await;
}

#[tokio::test]
async fn postgres_login_key_reply_fails_after_sending_fence_on_last_renewal() {
    let database = support::TestDatabase::create().await.unwrap();
    let store = database.store().await.unwrap();
    let (user_id, claim) = register_reply(&store).await;
    let prepared = expectation(&claim);
    let outcome = store
        .start_sending_authorized(claim.clone(), Timestamp::now(), None, Some(&prepared))
        .await
        .unwrap();
    assert_matches!(outcome, DeliveryFenceOutcome::Sending);
    rename(&store, user_id).await;
    assert_binding_rejected(&store, &claim, "sending").await;
}

#[tokio::test]
async fn postgres_admin_login_key_retry_rejects_rebind_without_changing_failed_or_unknown_result() {
    for outcome_unknown in [false, true] {
        let database = support::TestDatabase::create().await.unwrap();
        let store = database.store().await.unwrap();
        let (user_id, claim) = register_reply(&store).await;
        let prepared = expectation(&claim);
        let outcome = store
            .start_sending_authorized(claim.clone(), Timestamp::now(), None, Some(&prepared))
            .await
            .unwrap();
        assert_matches!(outcome, DeliveryFenceOutcome::Sending);
        DeliveryWorkStore::fail(
            &store,
            claim.clone(),
            DeliveryTransportError {
                failure_code: "fixture_failure".to_owned(),
                safe_detail: "测试失败".to_owned(),
                outcome_unknown,
                external_reference: None,
                result_metadata: serde_json::json!({"outcome_unknown": outcome_unknown}),
            },
            Timestamp::now(),
        )
        .await
        .unwrap();
        let action = if outcome_unknown {
            DeliveryAdminAction::ReconcileRetry
        } else {
            DeliveryAdminAction::RetryFailed
        };
        let (before, guard, _) = store.detail_with_guard(claim.delivery_id).await.unwrap();
        assert_eq!(guard.projected_blocker(action), None);

        rename(&store, user_id).await;
        let command = DeliveryAdminCommand {
            command_id: Uuid::new_v4(),
            delivery_id: claim.delivery_id,
            expected_fencing_version: before.delivery.fencing_version,
            action,
            actor_id: "admin".to_owned(),
            request_id: None,
            request_hash: action.request_hash(
                claim.delivery_id,
                before.delivery.fencing_version,
                None,
                None,
                "admin",
            ),
            external_reference: None,
            failure_code: None,
        };
        assert_matches!(
            store.execute(command.clone()).await,
            Err(ApplicationError::DeliveryAdmin(DeliveryAdminError::RecipientRenamed))
        );
        assert_eq!(store.detail(claim.delivery_id).await.unwrap(), before);
        let ledger_entries: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.delivery_admin_commands WHERE command_id = $1",
        )
        .bind(command.command_id)
        .fetch_one(store.postgres_pool())
        .await
        .unwrap();
        assert_eq!(ledger_entries, 0);
    }
}
