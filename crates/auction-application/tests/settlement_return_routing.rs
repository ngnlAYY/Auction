//! 结算退件邮件的路由边界：被拒收的结算邮件由系统发件人 `MoogleMail` 以 `[RTS] ` + 原
//! canonical 主题送回，处理器只把它记成退件证据（`settlement_recovery_returned`），不触发外部
//! 动作；来源已认证的退件即使不可领取也只记证据、不阻塞收件游标。无前缀的结算主题、玩家自拟的
//! 同名主题与其他系统退信都按未知邮件处理。
//!
//! 只使用窄端口替身：不连接数据库、不访问 MoogleMail。

// 测试夹具的固定值失败即测试失败；不对测试装配伪造回退。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::future::Future;
use std::sync::{Arc, Mutex};

use auction_application::admin::AdminIntakeSourceSnapshot;
use auction_application::core::ApplicationFuture;
use auction_application::core::{ApplicationError, Clock};
use auction_application::mail::{
    MailAuctionAcceptance, MailAuctionIntakeCommand, MailAuctionIntakeLookup,
    MailAuctionIntakePort, MailAuctionIntakeResult, MailDeliveryActivation, MailDeliveryCommand,
    MailDeliveryEnqueuer, MailDeliveryOperation, MailDeliveryResult, MailEquipmentAttachment,
    MailEquipmentFilter, MailEquipmentReader, MailMessage, MailNoticeEnqueue, MailNoticeLimit,
    MailProcessingDecision, MailProcessor, MailProcessorConfig, MailProcessorService,
    MailReplyPolicy, MailReplyPolicySource, MailUserPort, MailUserRebindCommand,
    MailUserRegisterCommand, MailUserRekeyCommand, MailUserResult, SubmissionReceiptFacts,
};
use auction_domain::EquipmentKey;
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

const NOW: &str = "2026-11-04T12:00:00Z";
const OBLIGATION_ID: &str = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";
const EQUIPMENT_ID: &str = "3125357";

fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        at(NOW)
    }
}

#[derive(Debug)]
struct DefaultPolicy;

impl MailReplyPolicySource for DefaultPolicy {
    fn current(&self) -> MailReplyPolicy {
        MailReplyPolicy::default()
    }
}

/// 退件路由不读装备页；任何读取都说明邮件被误判成投稿。
struct NoEquipment;

impl MailEquipmentReader for NoEquipment {
    fn read(
        &self,
        _world_id: String,
        _attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("equipment")) })
    }
}

struct NoAuctions;

impl MailAuctionIntakePort for NoAuctions {
    fn find_accepted(
        &self,
        _lookup: MailAuctionIntakeLookup,
    ) -> ApplicationFuture<'_, Option<MailAuctionAcceptance>> {
        Box::pin(async { Ok(None) })
    }

    fn ingest_mail(
        &self,
        _command: MailAuctionIntakeCommand,
    ) -> ApplicationFuture<'_, MailAuctionIntakeResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("auction intake")) })
    }

    fn load_submission_receipt(
        &self,
        _submission_id: Uuid,
    ) -> ApplicationFuture<'_, Option<SubmissionReceiptFacts>> {
        Box::pin(async { Ok(None) })
    }
}

#[derive(Default)]
struct RecordingDeliveries {
    commands: Mutex<Vec<MailDeliveryCommand>>,
}

impl RecordingDeliveries {
    fn operations(&self) -> Vec<MailDeliveryOperation> {
        self.commands.lock().unwrap().iter().map(|command| command.operation).collect()
    }
}

impl MailDeliveryEnqueuer for RecordingDeliveries {
    fn enqueue(&self, command: MailDeliveryCommand) -> ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async move {
            self.commands.lock().unwrap().push(command);
            Ok(MailDeliveryResult { delivery_id: Uuid::new_v4() })
        })
    }

    fn enqueue_within_limit(
        &self,
        command: MailDeliveryCommand,
        _limit: MailNoticeLimit,
    ) -> ApplicationFuture<'_, MailNoticeEnqueue> {
        Box::pin(async move {
            self.commands.lock().unwrap().push(command);
            Ok(MailNoticeEnqueue::Enqueued(MailDeliveryResult { delivery_id: Uuid::new_v4() }))
        })
    }

    fn prepare_recovery(
        &self,
        _command: MailDeliveryCommand,
    ) -> ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("prepare recovery")) })
    }

    fn activate_recovery(
        &self,
        _activation: MailDeliveryActivation,
    ) -> ApplicationFuture<'_, MailDeliveryResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("activate recovery")) })
    }
}

/// 退件路由不触达用户命令。
struct NoUsers;

impl MailUserPort for NoUsers {
    fn register_or_recover(
        &self,
        _command: MailUserRegisterCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("register")) })
    }

    fn rebind_username(
        &self,
        _command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("rebind")) })
    }

    fn rekey_login_key(
        &self,
        _command: MailUserRekeyCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("rekey")) })
    }
}

struct Service {
    processor:
        MailProcessorService<NoEquipment, NoAuctions, RecordingDeliveries, NoUsers, FixedClock>,
    deliveries: Arc<RecordingDeliveries>,
}

fn service() -> Service {
    let deliveries = Arc::new(RecordingDeliveries::default());
    let processor = MailProcessorService::new(
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
        Arc::new(NoEquipment),
        Arc::new(NoAuctions),
        Arc::clone(&deliveries),
        Arc::new(NoUsers),
        Arc::new(FixedClock),
    )
    .expect("processor configuration");
    Service { processor, deliveries }
}

/// 系统退回的 COD 结算邮件；路由层把附件与金额原样记作证据，不判断退信是否保留原 COD。
fn returned_mail(sender: &str, subject: &str) -> MailMessage {
    MailMessage {
        mailbox_key: "mooglemail:main".to_owned(),
        external_mail_id: format!("mid-{sender}-{subject}"),
        world_id: "main".to_owned(),
        received_at: at(NOW),
        sender: sender.to_owned(),
        subject: subject.to_owned(),
        body: "This message was returned from BuyerPlayer, kupo!".to_owned(),
        credits: 0,
        cod: 125_000,
        attachments: vec![MailEquipmentAttachment {
            external_equipment_id: EQUIPMENT_ID.to_owned(),
            key: EquipmentKey::new("0123456789").unwrap(),
        }],
        returnable: true,
        claimable: true,
        content_digest: [9; 32],
    }
}

fn process(
    service: &Service,
    message: MailMessage,
) -> Result<MailProcessingDecision, ApplicationError> {
    block_on(service.processor.process(message)).result
}

/// 极简阻塞执行器；测试 Future 只包含就绪路径，不依赖 tokio。
fn block_on<F: Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut future = Box::pin(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

fn assert_settlement_return(decision: &MailProcessingDecision) {
    assert_eq!(
        (decision.mail_kind.as_str(), decision.status.as_str(), decision.decision_code.as_str()),
        ("delivery", "accepted", "settlement_recovery_returned")
    );
    assert_eq!(
        decision.metadata,
        json!({
            "obligation_id": OBLIGATION_ID,
            "credits": 0,
            "cod": 125_000,
            "attachment_ids": [EQUIPMENT_ID],
        })
    );
}

#[test]
fn system_returned_payment_request_is_settlement_return_evidence() {
    let service = service();
    let subject = format!("[RTS] Auction payment request [{OBLIGATION_ID}]");
    let decision = process(&service, returned_mail("MoogleMail", &subject)).expect("退件证据");
    assert_settlement_return(&decision);
    // 证据只交给 recovery Worker 严格比对；处理器不退回、不领取、不回信。
    assert!(service.deliveries.operations().is_empty());
}

#[test]
fn system_returned_equipment_delivery_matches_case_variants() {
    let service = service();
    let upper_id = OBLIGATION_ID.to_ascii_uppercase();
    for subject in [
        format!("[RTS] Auction equipment delivery [{OBLIGATION_ID}]"),
        format!("[rts] AUCTION EQUIPMENT DELIVERY [{upper_id}]"),
    ] {
        let decision = process(&service, returned_mail("MoogleMail", &subject)).expect("退件证据");
        assert_settlement_return(&decision);
    }
    assert!(service.deliveries.operations().is_empty());
}

#[test]
fn player_rts_subject_is_not_settlement_return_evidence() {
    let service = service();
    let subject = format!("[RTS] Auction payment request [{OBLIGATION_ID}]");
    let decision =
        process(&service, returned_mail("BuyerPlayer", &subject)).expect("未知邮件按原路径收口");
    // 玩家不能冒充系统发件人：带资产的同名主题按未知邮件整封退回，不形成退件证据。
    assert_eq!(
        (decision.mail_kind.as_str(), decision.decision_code.as_str()),
        ("unknown", "unknown_assets_rejected")
    );
    assert_eq!(service.deliveries.operations(), [MailDeliveryOperation::Return]);
}

#[test]
fn other_system_returns_keep_the_unknown_path() {
    let service = service();
    let mut message = returned_mail("MoogleMail", "[RTS] Auction payment overdue");
    message.cod = 0;
    message.attachments.clear();
    let decision = process(&service, message).expect("无资产系统退信归档");
    assert_eq!(
        (decision.mail_kind.as_str(), decision.status.as_str(), decision.decision_code.as_str()),
        ("unknown", "accepted", "archived")
    );
    assert!(service.deliveries.operations().is_empty());
}

/// 来源已认证的退件即使不可领取（例如没有资产）也只记证据，由消费方隔离；不能以
/// `UnsafeDisposition` 让收件游标停在这封邮件上。
#[test]
fn unclaimable_system_return_is_recorded_without_blocking() {
    let service = service();
    let subject = format!("[RTS] Auction payment request [{OBLIGATION_ID}]");
    let mut message = returned_mail("MoogleMail", &subject);
    message.claimable = false;
    message.returnable = false;
    message.cod = 0;
    message.attachments.clear();
    let decision = process(&service, message).expect("已认证的退件不能阻塞游标");
    assert_eq!(
        (decision.mail_kind.as_str(), decision.status.as_str(), decision.decision_code.as_str()),
        ("delivery", "accepted", "settlement_recovery_returned")
    );
    assert_eq!(
        decision.metadata,
        json!({"obligation_id": OBLIGATION_ID, "credits": 0, "cod": 0, "attachment_ids": []})
    );
    assert!(service.deliveries.operations().is_empty());
}

/// 系统退回的卖方货款与买方退款同样只记为退件证据，带 Credits 也不会被当作未知邮件退回。
#[test]
fn system_returned_payout_and_refund_are_settlement_return_evidence() {
    let service = service();
    for base in ["Auction seller payout", "Auction buyer refund"] {
        let mut message = returned_mail("MoogleMail", &format!("[RTS] {base} [{OBLIGATION_ID}]"));
        message.credits = 250_000;
        message.cod = 0;
        message.attachments.clear();
        let decision = process(&service, message).expect("货款或退款退件证据");
        assert_eq!(
            (decision.mail_kind.as_str(), decision.decision_code.as_str()),
            ("delivery", "settlement_recovery_returned"),
            "{base}"
        );
        assert_eq!(
            decision.metadata,
            json!({
                "obligation_id": OBLIGATION_ID,
                "credits": 250_000,
                "cod": 0,
                "attachment_ids": [],
            })
        );
    }
    assert!(service.deliveries.operations().is_empty());
}

/// 无前缀兼容入口已撤掉。任何发件人寄来的 canonical 结算主题都按未知邮件处理：带资产
/// 整封退回、不带资产归档，绝不形成退件证据（义务 ID 可由公开 ID 推出，不能证明来源）。
#[test]
fn prefixless_settlement_subject_is_never_return_evidence() {
    let service = service();
    for sender in ["BuyerPlayer", "MoogleMail"] {
        let subject = format!("Auction equipment delivery [{OBLIGATION_ID}]");
        let decision =
            process(&service, returned_mail(sender, &subject)).expect("未知邮件按原路径收口");
        assert_eq!(
            (decision.mail_kind.as_str(), decision.decision_code.as_str()),
            ("unknown", "unknown_assets_rejected"),
            "{sender}"
        );
        let mut assetless =
            returned_mail(sender, &format!("auction payment request [{OBLIGATION_ID}]"));
        assetless.cod = 0;
        assetless.attachments.clear();
        let decision = process(&service, assetless).expect("无资产未知邮件归档");
        assert_eq!(
            (decision.mail_kind.as_str(), decision.decision_code.as_str()),
            ("unknown", "archived"),
            "{sender}"
        );
    }
    assert_eq!(
        service.deliveries.operations(),
        [MailDeliveryOperation::Return, MailDeliveryOperation::Return]
    );
}
