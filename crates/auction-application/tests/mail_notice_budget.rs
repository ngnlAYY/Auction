//! W9.10/W9.11 邮件回信预算与登录 key 轮换的边界测试。
//!
//! 只使用窄端口替身：不连接数据库、不访问 MoogleMail。覆盖
//! 1) `rekey` 命令结果写入后只回信新 key；
//! 2) 24 小时账号回信预算耗尽时仍记录命令但不再寄信；
//! 3) 说明信预算耗尽时仍退回附带资产。

// 测试夹具的固定值失败即测试失败；不对测试装配伪造回退。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::future::Future;
use std::sync::{Arc, Mutex};

use auction_application::admin::AdminIntakeSourceSnapshot;
use auction_application::auction::ApplicationFuture;
use auction_application::core::{ApplicationError, Clock, RequestHash};
use auction_application::mail::{
    MailAuctionAcceptance, MailAuctionIntakeCommand, MailAuctionIntakeLookup,
    MailAuctionIntakePort, MailAuctionIntakeResult, MailDeliveryActivation, MailDeliveryCommand,
    MailDeliveryEnqueuer, MailDeliveryResult, MailEquipmentAttachment, MailEquipmentReader,
    MailLoginKeyEnvelope, MailLoginKeyIssue, MailLoginKeyIssuer, MailMessage, MailNoticeEnqueue,
    MailNoticeLimit, MailNotificationTemplate, MailProcessor, MailProcessorConfig,
    MailProcessorService, MailUserCommandKind, MailUserPort, MailUserRebindCommand,
    MailUserRegisterCommand, MailUserRekeyCommand, MailUserRekeyWrite, MailUserResult,
    MailUserResultCode, MailUserStore,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

const NOW: &str = "2026-11-04T12:00:00Z";

fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value).unwrap().with_timezone(&Utc)
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        at(NOW)
    }
}

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
    ) -> ApplicationFuture<'_, Option<auction_application::mail::SubmissionReceiptFacts>> {
        Box::pin(async { Ok(None) })
    }
}

#[derive(Default)]
struct RecordingDeliveries {
    commands: Mutex<Vec<MailDeliveryCommand>>,
    suppressed: Mutex<bool>,
}

impl RecordingDeliveries {
    fn recorded(&self) -> Vec<MailDeliveryCommand> {
        self.commands.lock().unwrap().clone()
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
            if *self.suppressed.lock().unwrap() {
                return Ok(MailNoticeEnqueue::Suppressed);
            }
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

struct RecordingUsers {
    calls: Mutex<Vec<MailUserCommandKind>>,
    user_id: Uuid,
}

impl RecordingUsers {
    fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()), user_id: Uuid::from_u128(7) }
    }
}

impl MailUserPort for RecordingUsers {
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
        self.calls.lock().unwrap().push(MailUserCommandKind::Rebind);
        let user_id = self.user_id;
        Box::pin(async move {
            Ok(MailUserResult { user_id: Some(user_id), code: MailUserResultCode::Rebound })
        })
    }

    fn rekey_login_key(
        &self,
        _command: MailUserRekeyCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        self.calls.lock().unwrap().push(MailUserCommandKind::Rekey);
        let user_id = self.user_id;
        Box::pin(async move {
            Ok(MailUserResult { user_id: Some(user_id), code: MailUserResultCode::Rekeyed })
        })
    }
}

struct Service {
    processor: MailProcessorService<
        NoEquipment,
        NoAuctions,
        RecordingDeliveries,
        RecordingUsers,
        FixedClock,
    >,
    users: Arc<RecordingUsers>,
    deliveries: Arc<RecordingDeliveries>,
}

fn service(suppressed: bool) -> Service {
    let users = Arc::new(RecordingUsers::new());
    let deliveries = Arc::new(RecordingDeliveries {
        commands: Mutex::new(Vec::new()),
        suppressed: Mutex::new(suppressed),
    });
    let processor = MailProcessorService::new_for_worlds(
        vec!["main".to_owned()],
        MailProcessorConfig {
            start_price: 50_000,
            equipment_filter: auction_application::mail::MailEquipmentFilter {
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
            account_reply_window_seconds: 86_400,
            explanation_window_seconds: 86_400,
            explanation_limit: 10,
            rekey_enabled: true,
        },
        Arc::new(NoEquipment),
        Arc::new(NoAuctions),
        Arc::clone(&deliveries),
        Arc::clone(&users) as Arc<_>,
        Arc::new(FixedClock),
    )
    .expect("processor configuration");
    Service { processor, users, deliveries }
}

fn message(subject: &str, sender: &str, credits: i64) -> MailMessage {
    MailMessage {
        mailbox_key: "mooglemail:main".to_owned(),
        external_mail_id: format!("mid-{subject}-{credits}"),
        world_id: "main".to_owned(),
        received_at: at(NOW),
        sender: sender.to_owned(),
        subject: subject.to_owned(),
        body: String::new(),
        credits,
        cod: 0,
        attachments: Vec::new(),
        returnable: credits > 0,
        claimable: false,
        content_digest: [7; 32],
    }
}

fn run(
    service: &Service,
    message: MailMessage,
) -> auction_application::mail::MailProcessingDecision {
    let attempt = futures_lite_block_on(service.processor.process(message));
    attempt.result.expect("processing decision")
}

/// 极简阻塞执行器；测试 Future 只包含就绪路径，不依赖 tokio。
fn futures_lite_block_on<F: Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::yield_now();
    }
}

#[test]
fn rekey_sends_the_new_key_and_uses_the_account_cadence_template() {
    let service = service(false);
    let decision = run(&service, message("rekey", "ReKeyPlayer", 0));

    assert_eq!(decision.mail_kind, "rekey");
    assert_eq!(decision.decision_code, "rekey_accepted");
    assert_eq!(service.users.calls.lock().unwrap().as_slice(), &[MailUserCommandKind::Rekey]);
    let commands = service.deliveries.recorded();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].template, Some(MailNotificationTemplate::RekeyKey));
    assert_eq!(commands[0].result_code.as_deref(), Some("rekeyed"));
    assert_eq!(commands[0].user_key_user_id, Some(Uuid::from_u128(7)));
    assert_eq!(decision.metadata["notice_suppressed"], false);
}

#[test]
fn exhausted_account_window_still_records_the_command_without_mailing() {
    let service = service(true);
    let decision = run(&service, message("rekey", "ReKeyPlayer", 0));

    assert_eq!(decision.decision_code, "rekey_accepted");
    assert_eq!(service.users.calls.lock().unwrap().len(), 1, "轮换命令仍必须提交");
    assert!(service.deliveries.recorded().is_empty(), "预算耗尽时不得入队回信");
    assert_eq!(decision.metadata["notice_suppressed"], true);
    assert_eq!(decision.metadata["delivery_ids"].as_array().map(Vec::len), Some(0));
}

#[test]
fn explanation_cap_keeps_the_asset_return_without_the_notice() {
    let service = service(true);
    let decision = run(&service, message("rebind", "AssetSender", 100));

    assert_eq!(decision.decision_code, "user_assets_rejected");
    let commands = service.deliveries.recorded();
    assert_eq!(commands.len(), 1, "退回操作必须仍然入队");
    assert_eq!(commands[0].operation, auction_application::mail::MailDeliveryOperation::Return);
    assert_eq!(decision.metadata["notice_suppressed"], true);
    assert_eq!(decision.metadata["delivery_ids"].as_array().map(Vec::len), Some(1));
}

// ---- 服务层：轮换写入确实拿到新摘要/信封 -------------------------------

struct StubStore {
    write: Mutex<Option<MailUserRekeyWrite>>,
}

impl MailUserStore for StubStore {
    fn find_registration_user_id(&self, _username: String) -> ApplicationFuture<'_, Option<Uuid>> {
        Box::pin(async { Ok(Some(Uuid::from_u128(9))) })
    }

    fn find_mail_command(
        &self,
        _command_id: auction_domain::CommandId,
        _kind: MailUserCommandKind,
        _request_hash: RequestHash,
    ) -> ApplicationFuture<'_, Option<MailUserResult>> {
        Box::pin(async { Ok(None) })
    }

    fn register_or_recover(
        &self,
        _write: auction_application::mail::MailUserRegisterWrite,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("register")) })
    }

    fn rebind_username(
        &self,
        _command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("rebind")) })
    }

    fn rekey_login_key(&self, write: MailUserRekeyWrite) -> ApplicationFuture<'_, MailUserResult> {
        let user_id = write.user_id;
        *self.write.lock().unwrap() = Some(write);
        Box::pin(async move {
            Ok(MailUserResult { user_id: Some(user_id), code: MailUserResultCode::Rekeyed })
        })
    }
}

struct StubIssuer;

impl MailLoginKeyIssuer for StubIssuer {
    fn issue(&self, _user_id: Uuid) -> ApplicationFuture<'_, MailLoginKeyIssue> {
        Box::pin(async {
            Ok(MailLoginKeyIssue {
                public_key_hash: [42; 32],
                envelope: MailLoginKeyEnvelope::new(
                    vec![1; 24],
                    vec![2; 12],
                    "test-key".to_owned(),
                    "fingerprint".to_owned(),
                )
                .unwrap(),
            })
        })
    }
}

#[test]
fn rekey_service_writes_a_fresh_envelope_under_the_locked_user_id() {
    let store = Arc::new(StubStore { write: Mutex::new(None) });
    let service =
        auction_application::mail::MailUserService::new(Arc::clone(&store), Arc::new(StubIssuer));
    let command = MailUserRekeyCommand {
        command_id: auction_domain::CommandId::from_uuid(Uuid::from_u128(11)),
        request_hash: RequestHash::new([3; 32]),
        username: "ReKeyPlayer".to_owned(),
        now: at(NOW),
    };
    let result = futures_lite_block_on(service.rekey_login_key(command)).expect("rekey result");
    assert_eq!(result.code, MailUserResultCode::Rekeyed);
    let write = store.write.lock().unwrap().take().expect("store must receive rekey write");
    assert_eq!(write.user_id, Uuid::from_u128(9));
    assert_eq!(write.public_key_hash, [42; 32]);
    assert!(write.envelope.ciphertext().len() >= 16);
}
