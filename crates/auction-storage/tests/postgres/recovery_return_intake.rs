//! 系统退件从真实邮件处理器到恢复消费的贯通（PostgreSQL 18）。
//!
//! 被拒收的 COD 带完整系统后缀退回：邮件处理器把原收件人与原正文摘要一并记入证据，证据经
//! `MailPollStore::commit` 原样持久化，恢复流程再按同一严格模型消费并建立收回动作，队列继续
//! 处理下一封。此前恢复侧只认四个字段，处理器一写正文事实，消费就在队头每轮报错。
//!
//! 处理器只接窄端口替身：退件路由不读装备页、不入队投递、不触达用户命令。仅在 `test-db`
//! feature 下编译，入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration as StdDuration;

use auction_application::admin::AdminIntakeSourceSnapshot;
use auction_application::core::{ApplicationError, ApplicationFuture, Clock};
use auction_application::mail::{
    MailAuctionAcceptance, MailAuctionIntakeCommand, MailAuctionIntakeLookup,
    MailAuctionIntakePort, MailAuctionIntakeResult, MailDeliveryActivation, MailDeliveryCommand,
    MailDeliveryEnqueuer, MailDeliveryResult, MailEquipmentAttachment, MailEquipmentFilter,
    MailEquipmentReader, MailMessage, MailNoticeEnqueue, MailNoticeLimit, MailPollBatch,
    MailPollIntake, MailPollStore, MailProcessor, MailProcessorConfig, MailProcessorService,
    MailReplyPolicy, MailReplyPolicySource, MailUserPort, MailUserRebindCommand,
    MailUserRegisterCommand, MailUserRekeyCommand, MailUserResult, SubmissionReceiptFacts,
};
use auction_application::recovery::RecoveryWorkStore;
use auction_domain::EquipmentKey;
use auction_storage::DatabaseStore;
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::recovery_returns::{
    CodSale, consumed_returns, insert_awaiting_recovery, insert_sale, settlement_minute,
};
use crate::support;

const MAILBOX: &str = "mooglemail:main";

/// 退件路由不读时钟以外的任何依赖；固定时刻只用于其它路由。
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Debug)]
struct DefaultPolicy;

impl MailReplyPolicySource for DefaultPolicy {
    fn current(&self) -> MailReplyPolicy {
        MailReplyPolicy::default()
    }
}

/// 处理器的四个窄端口替身：退件路由触达任何一个都说明邮件被误判，调用即返回能力不可用。
struct Untouched;

fn untouched<T>() -> ApplicationFuture<'static, T> {
    Box::pin(async { Err(ApplicationError::CapabilityUnavailable("退件路由不应触达")) })
}

impl MailEquipmentReader for Untouched {
    fn read(
        &self,
        _world_id: String,
        _attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        untouched()
    }
}

impl MailAuctionIntakePort for Untouched {
    fn find_accepted(
        &self,
        _lookup: MailAuctionIntakeLookup,
    ) -> ApplicationFuture<'_, Option<MailAuctionAcceptance>> {
        // 每封邮件都会先查投稿接受事实；退件从未被接受为投稿。
        Box::pin(async { Ok(None) })
    }

    fn ingest_mail(
        &self,
        _command: MailAuctionIntakeCommand,
    ) -> ApplicationFuture<'_, MailAuctionIntakeResult> {
        untouched()
    }

    fn load_submission_receipt(
        &self,
        _submission_id: Uuid,
    ) -> ApplicationFuture<'_, Option<SubmissionReceiptFacts>> {
        untouched()
    }
}

impl MailDeliveryEnqueuer for Untouched {
    fn enqueue(&self, _command: MailDeliveryCommand) -> ApplicationFuture<'_, MailDeliveryResult> {
        untouched()
    }

    fn enqueue_within_limit(
        &self,
        _command: MailDeliveryCommand,
        _limit: MailNoticeLimit,
    ) -> ApplicationFuture<'_, MailNoticeEnqueue> {
        untouched()
    }

    fn prepare_recovery(
        &self,
        _command: MailDeliveryCommand,
    ) -> ApplicationFuture<'_, MailDeliveryResult> {
        untouched()
    }

    fn activate_recovery(
        &self,
        _activation: MailDeliveryActivation,
    ) -> ApplicationFuture<'_, MailDeliveryResult> {
        untouched()
    }
}

impl MailUserPort for Untouched {
    fn register_or_recover(
        &self,
        _command: MailUserRegisterCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        untouched()
    }

    fn rebind_username(
        &self,
        _command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        untouched()
    }

    fn rekey_login_key(
        &self,
        _command: MailUserRekeyCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        untouched()
    }
}

type Processor = MailProcessorService<Untouched, Untouched, Untouched, Untouched, FixedClock>;

fn processor(now: DateTime<Utc>) -> Processor {
    MailProcessorService::new(
        vec!["main".to_owned()],
        MailProcessorConfig {
            start_price: 50_000,
            equipment_filter: MailEquipmentFilter {
                enabled: false,
                qualities: Vec::new(),
                prefixes: Vec::new(),
                suffixes: Vec::new(),
                categories: Vec::new(),
                equipment_types: Vec::new(),
                level_min: 0,
                level_max: 0,
                early_bird: None,
            },
            rekey_enabled: true,
            reply_policy: Arc::new(DefaultPolicy),
        },
        Arc::new(Untouched),
        Arc::new(Untouched),
        Arc::new(Untouched),
        Arc::new(Untouched),
        Arc::new(FixedClock(now)),
    )
    .expect("处理器配置")
}

/// 系统退回的装备 COD：原正文原样带回并追加系统后缀；退件只带回装备，不保留 COD。
fn returned_cod(sale: &CodSale, external_mail_id: &str, received_at: DateTime<Utc>) -> MailMessage {
    let original = format!(
        "Your auction item is attached as COD.\n\nAmount: 100 Credits\n\nReference: {}",
        sale.payment_obligation
    );
    MailMessage {
        mailbox_key: MAILBOX.to_owned(),
        external_mail_id: external_mail_id.to_owned(),
        world_id: "main".to_owned(),
        received_at,
        sender: "MoogleMail".to_owned(),
        subject: format!("[RTS] Auction equipment delivery [{}]", sale.payment_obligation),
        body: format!("{original}\n\nThis message was returned from BuyerPlayer, kupo!"),
        credits: 0,
        cod: 0,
        attachments: vec![MailEquipmentAttachment {
            external_equipment_id: sale.external_equipment_id.clone(),
            key: EquipmentKey::new("0123456789").expect("装备 key"),
        }],
        returnable: true,
        claimable: true,
        content_digest: [0x5a; 32],
    }
}

/// 按 Worker 的映射把处理器决定转成入站事实：身份、头部与决定原样写入。
async fn intake(message: MailMessage, processor: &Processor, now: DateTime<Utc>) -> MailPollIntake {
    let id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!(
            "auction/inbound-mail-v1/{MAILBOX}/{}/{}",
            message.world_id, message.external_mail_id
        )
        .as_bytes(),
    );
    let external_mail_id = message.external_mail_id.clone();
    let sender = message.sender.clone();
    let subject = message.subject.clone();
    let world_id = message.world_id.clone();
    let content_digest = message.content_digest;
    let received_at = message.received_at;
    let decision = processor.process(message).await.result.expect("系统退件必须形成证据");
    assert_eq!(
        (decision.mail_kind.as_str(), decision.status.as_str(), decision.decision_code.as_str()),
        ("delivery", "accepted", "settlement_recovery_returned")
    );
    // 用例的前提：处理器确实写入了系统后缀的正文事实，而不是旧的四字段证据。
    assert_eq!(decision.metadata["returned_from"], "BuyerPlayer");
    assert!(decision.metadata["body_digest"].as_str().is_some_and(|digest| digest.len() == 64));
    MailPollIntake {
        id,
        external_mail_id,
        sender: Some(sender),
        subject: Some(subject),
        world_id,
        mail_kind: decision.mail_kind,
        status: decision.status,
        content_digest,
        received_at,
        decision_code: decision.decision_code,
        decision_metadata: decision.metadata,
        message_display: None,
        equipment_diagnostic: None,
        processed_at: now,
    }
}

/// 经生产端口领取邮箱租约并原子提交整批入站事实。
async fn commit(store: &DatabaseStore, items: Vec<MailPollIntake>, now: DateTime<Utc>) {
    store
        .ensure_state(
            MAILBOX.to_owned(),
            now,
            now + Duration::minutes(1),
            StdDuration::from_secs(3600),
        )
        .await
        .expect("初始化邮箱状态");
    let claim = MailPollStore::claim_due(
        store,
        MAILBOX.to_owned(),
        "return-intake-worker".to_owned(),
        now,
        now + Duration::minutes(2),
        1,
        true,
    )
    .await
    .expect("领取邮箱")
    .pop()
    .expect("强制领取必须返回邮箱租约");
    let next_cursor = items.last().map(|item| item.external_mail_id.clone());
    let batch = MailPollBatch {
        mailbox_key: claim.mailbox_key.clone(),
        claim_token: claim.claim_token,
        next_cursor,
        polled_at: now,
        next_poll_at: now + Duration::minutes(1),
        items,
    };
    MailPollStore::commit(store, claim, batch).await.expect("提交入站批次");
}

#[tokio::test]
async fn postgres_processor_return_evidence_is_consumed_by_recovery() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let first_sale = insert_sale(pool, 7_400_001, settled_at, 1).await;
    let first_recovery = insert_awaiting_recovery(pool, &first_sale, settled_at).await;
    let second_sale = insert_sale(pool, 7_400_002, settled_at, 1).await;
    let second_recovery = insert_awaiting_recovery(pool, &second_sale, settled_at).await;

    let now = Utc::now();
    let processor = processor(now);
    let first = returned_cod(&first_sale, "990101", minute + Duration::minutes(30));
    let second = returned_cod(&second_sale, "990102", minute + Duration::minutes(31));
    let items = vec![intake(first, &processor, now).await, intake(second, &processor, now).await];
    let first_mail = items[0].id;
    let second_mail = items[1].id;
    commit(&store, items, now).await;

    // 队头是第一封：按共享严格模型消费，证据与冻结预期一致，建立收回动作。
    for (recovery_id, mail_id, sale) in
        [(first_recovery, first_mail, &first_sale), (second_recovery, second_mail, &second_sale)]
    {
        let advance = store
            .record_due_return(Utc::now())
            .await
            .expect("带正文事实的系统退件必须被恢复流程消费")
            .expect("必须领取到排在队头的退件");
        assert_eq!(advance.recovery_id, recovery_id.into());
        assert_eq!(advance.return_mail_id, mail_id);
        assert_eq!(advance.stage, "reclaiming");
        assert!(advance.blocking_code.is_none(), "证据与冻结预期一致，不得隔离");
        assert_eq!(advance.action_kinds, vec!["reclaim_equipment_take".to_owned()]);
        assert_eq!(consumed_returns(pool, recovery_id).await, 1);
        let attachments: Vec<String> = sqlx::query_scalar(
            "SELECT expected_attachment_ids FROM public.settlement_recovery_actions
             WHERE recovery_id = $1 AND kind = 'reclaim_equipment_take'",
        )
        .bind(recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取收回动作");
        assert_eq!(attachments, vec![sale.external_equipment_id.clone()]);
    }
    assert!(
        store.record_due_return(Utc::now()).await.expect("队列排空不报错").is_none(),
        "两封退件都已消费"
    );

    store.postgres_pool().close().await;
}

/// 恢复记录的付款义务只有首寄：证据带派发引用时不能证明退回的是本记录那次发送，按内容不符隔离
/// （消费、`evidence_mismatch` 阻断、不建动作），队列不因此停住。
#[tokio::test]
async fn postgres_recovery_return_with_dispatch_reference_is_isolated() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let (minute, settled_at) = settlement_minute();
    let sale = insert_sale(pool, 7_400_101, settled_at, 1).await;
    let recovery_id = insert_awaiting_recovery(pool, &sale, settled_at).await;

    let now = Utc::now();
    let processor = processor(now);
    let mut message = returned_cod(&sale, "990201", minute + Duration::minutes(30));
    let reference =
        auction_domain::SettlementDispatchReference::for_resend(sale.payment_obligation, 2)
            .expect("补寄派发引用");
    message.body = format!(
        "Your auction item is attached as COD.\n\nDispatch reference: {}\n\nThis message was returned from BuyerPlayer, kupo!",
        reference.encode()
    );
    let item = intake(message, &processor, now).await;
    assert_eq!(item.decision_metadata["dispatch_reference"], reference.encode());
    commit(&store, vec![item], now).await;

    let advance = store
        .record_due_return(Utc::now())
        .await
        .expect("带派发引用的退件必须隔离而不是报错")
        .expect("必须领取到该退件");
    assert_eq!(advance.recovery_id, recovery_id.into());
    assert_eq!(advance.blocking_code.as_deref(), Some("evidence_mismatch"));
    assert!(advance.action_kinds.is_empty());
    assert_eq!(consumed_returns(pool, recovery_id).await, 1);

    store.postgres_pool().close().await;
}
