//! 装备投稿被拒收时必须收口为整封退回加说明信，而不是永久卡住邮件游标：已登记装备、赛季前
//! 停收与装备来源无效三条处理器路径。投递替身按统一投递判定核对，与存储入队校验同一口径：
//! 拒收原因漏登记或载荷越界时这里得到与生产相同的入队失败。另覆盖已接受投稿的领取命令同样
//! 落在统一判定内。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::future::Future;
use std::sync::{Arc, Mutex};

use auction_application::admin::{AdminIntakeError, AdminIntakeSourceSnapshot};
use auction_application::core::ApplicationFuture;
use auction_application::core::{
    ApplicationError, Clock, CompetitionTransactionError, RepositoryError,
};
use auction_application::mail::{
    EquipmentRejectionReason, MailAuctionAcceptance, MailAuctionIntakeCommand,
    MailAuctionIntakeLookup, MailAuctionIntakePort, MailAuctionIntakeResult,
    MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation,
    MailDeliveryResult, MailEquipmentAttachment, MailEquipmentReader, MailMessage,
    MailNoticeEnqueue, MailNoticeLimit, MailNotificationTemplate, MailProcessingDecision,
    MailProcessor, MailProcessorConfig, MailProcessorService, MailReplyPolicy,
    MailReplyPolicySource, MailUserCommandKind, MailUserPort, MailUserRebindCommand,
    MailUserRegisterCommand, MailUserRekeyCommand, MailUserResult,
};
use auction_domain::EquipmentKey;
use auction_time::Timestamp;
use serde_json::json;
use uuid::Uuid;

const NOW: &str = "2026-11-04T12:00:00Z";

fn at(value: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(value).unwrap()
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        at(NOW)
    }
}

/// 回信预算固定为默认值；本用例只关心拒收的收口。
#[derive(Debug)]
struct DefaultPolicy;

impl MailReplyPolicySource for DefaultPolicy {
    fn current(&self) -> MailReplyPolicy {
        MailReplyPolicy::default()
    }
}

/// 装备来源：正常读回快照，或明确报告来源无效。
struct Equipment {
    invalid_source: bool,
}

impl MailEquipmentReader for Equipment {
    fn read(
        &self,
        _world_id: String,
        _attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        let invalid_source = self.invalid_source;
        Box::pin(async move {
            if invalid_source {
                return Err(ApplicationError::AdminIntake(AdminIntakeError::InvalidSource));
            }
            Ok(AdminIntakeSourceSnapshot {
                adapter: "test".to_owned(),
                source_fingerprint: [7; 32],
                external_equipment_id: "900000001".to_owned(),
                name: "Legendary Ethereal Great Mace of the Vampire".to_owned(),
                category: "Two-Handed".to_owned(),
                level: Some(100),
                tradeable: true,
                attributes: json!({
                    "type": "Two-Handed",
                    "quality": "Legendary",
                    "prefix": "Ethereal",
                    "suffix": "of the Vampire",
                    "tradeable": true,
                    "level": 100,
                    "stats": {}
                }),
                collected_at: at(NOW),
                key: EquipmentKey::new("0123456789").unwrap(),
            })
        })
    }
}

/// 录入事务的确定性拒绝；`CompetitionTransactionError` 不可克隆，按选择逐次构造。
#[derive(Clone, Copy)]
enum IntakeRefusal {
    EquipmentConflict,
    IntakeClosed,
}

struct RefusingAuctions(IntakeRefusal);

impl MailAuctionIntakePort for RefusingAuctions {
    fn find_accepted(
        &self,
        _lookup: MailAuctionIntakeLookup,
    ) -> ApplicationFuture<'_, Option<auction_application::mail::MailAuctionAcceptance>> {
        Box::pin(async { Ok(None) })
    }

    fn ingest_mail(
        &self,
        _command: MailAuctionIntakeCommand,
    ) -> ApplicationFuture<'_, MailAuctionIntakeResult> {
        let error = match self.0 {
            IntakeRefusal::EquipmentConflict => {
                CompetitionTransactionError::AdminIntakeEquipmentConflict
            }
            IntakeRefusal::IntakeClosed => CompetitionTransactionError::IntakeClosed,
        };
        Box::pin(async move { Err(ApplicationError::CompetitionTransaction(error)) })
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
}

impl RecordingDeliveries {
    /// 与存储入队校验同一口径：命令必须落在统一投递判定内，否则入队失败。
    fn record(&self, command: MailDeliveryCommand) -> Result<(), ApplicationError> {
        if let Err(violation) = command.shape().check() {
            return Err(RepositoryError::Failed(format!(
                "MoogleMail delivery 载荷不在统一判定内：{violation}"
            ))
            .into());
        }
        self.commands.lock().unwrap().push(command);
        Ok(())
    }
}

impl MailDeliveryEnqueuer for RecordingDeliveries {
    fn enqueue(&self, command: MailDeliveryCommand) -> ApplicationFuture<'_, MailDeliveryResult> {
        let recorded = self.record(command);
        Box::pin(async move {
            recorded?;
            Ok(MailDeliveryResult { delivery_id: Uuid::new_v4() })
        })
    }

    fn enqueue_within_limit(
        &self,
        command: MailDeliveryCommand,
        _limit: MailNoticeLimit,
    ) -> ApplicationFuture<'_, MailNoticeEnqueue> {
        let recorded = self.record(command);
        Box::pin(async move {
            recorded?;
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

struct NoUsers;

impl MailUserPort for NoUsers {
    fn register_or_recover(
        &self,
        _command: MailUserRegisterCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("users")) })
    }

    fn rebind_username(
        &self,
        _command: MailUserRebindCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("users")) })
    }

    fn rekey_login_key(
        &self,
        _command: MailUserRekeyCommand,
    ) -> ApplicationFuture<'_, MailUserResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("users")) })
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
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

/// 已接受过的投稿：重放时读回原接受结果，处理器只补领取与回执（回执事实缺失时跳过）。
struct AcceptedAuctions;

impl MailAuctionIntakePort for AcceptedAuctions {
    fn find_accepted(
        &self,
        _lookup: MailAuctionIntakeLookup,
    ) -> ApplicationFuture<'_, Option<MailAuctionAcceptance>> {
        Box::pin(async {
            Ok(Some(MailAuctionAcceptance {
                result: MailAuctionIntakeResult {
                    auction_id: Uuid::from_u128(1),
                    auction_sequence: 1,
                    auction_state_version: 1,
                    preview_revision: 0,
                    submission_id: Uuid::from_u128(2),
                    added_count: 1,
                    existing_count: 0,
                    lot_ids: vec![Uuid::from_u128(3)],
                },
                attachment_ids: vec!["900000001".to_owned()],
            }))
        })
    }

    fn ingest_mail(
        &self,
        _command: MailAuctionIntakeCommand,
    ) -> ApplicationFuture<'_, MailAuctionIntakeResult> {
        Box::pin(async { Err(ApplicationError::CapabilityUnavailable("ingest mail")) })
    }

    fn load_submission_receipt(
        &self,
        _submission_id: Uuid,
    ) -> ApplicationFuture<'_, Option<auction_application::mail::SubmissionReceiptFacts>> {
        Box::pin(async { Ok(None) })
    }
}

/// 处理一封带一件装备的 `auction` 邮件，返回处理决定与入队的全部 delivery 命令。
fn process<A: MailAuctionIntakePort + 'static>(
    equipment: Equipment,
    auctions: A,
) -> (MailProcessingDecision, Vec<MailDeliveryCommand>) {
    let deliveries = Arc::new(RecordingDeliveries::default());
    let processor = MailProcessorService::new(
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
            rekey_enabled: true,
            reply_policy: Arc::new(DefaultPolicy),
        },
        Arc::new(equipment),
        Arc::new(auctions),
        Arc::clone(&deliveries),
        Arc::new(NoUsers),
        Arc::new(FixedClock),
    )
    .expect("processor configuration");
    let message = MailMessage {
        mailbox_key: "mooglemail:main".to_owned(),
        external_mail_id: "7000002".to_owned(),
        world_id: "main".to_owned(),
        received_at: at(NOW),
        sender: "seller01".to_owned(),
        subject: "auction".to_owned(),
        body: "========== Attachment ==========\n\n[900000001] Legendary Ethereal Great Mace of the Vampire\n\n================================".to_owned(),
        credits: 0,
        cod: 0,
        attachments: vec![MailEquipmentAttachment {
            external_equipment_id: "900000001".to_owned(),
            key: EquipmentKey::new("34c20d12dc").unwrap(),
        }],
        returnable: true,
        claimable: true,
        content_digest: [9; 32],
    };
    let decision =
        block_on(processor.process(message)).result.expect("must be rejected, not error");
    let commands = deliveries.commands.lock().unwrap().clone();
    (decision, commands)
}

/// 拒收决定与退回、说明信两条 delivery；说明信的结果码就是决定中的原因。
fn assert_rejected_with(
    decision: &MailProcessingDecision,
    commands: &[MailDeliveryCommand],
    reason: EquipmentRejectionReason,
) {
    assert_eq!(decision.status, "rejected");
    assert_eq!(decision.decision_code, "auction_equipment_rejected");
    assert_eq!(decision.metadata["reason_code"], reason.as_str());
    assert_eq!(commands.len(), 2, "必须同时安排退回和说明信");
    assert!(commands.iter().any(|command| command.operation == MailDeliveryOperation::Return));
    assert!(commands.iter().any(|command| {
        command.template == Some(MailNotificationTemplate::AuctionEquipmentRejected)
            && command.result_code.as_deref() == Some(reason.as_str())
    }));
}

#[test]
fn duplicate_equipment_is_rejected_and_returned_instead_of_retried_forever() {
    let (decision, commands) = process(
        Equipment { invalid_source: false },
        RefusingAuctions(IntakeRefusal::EquipmentConflict),
    );
    assert_eq!(decision.metadata["reason_code"], "auction_equipment_conflict");
    assert_rejected_with(&decision, &commands, EquipmentRejectionReason::EquipmentConflict);
    // 用户状态/命令枚举只用于确保测试装配满足 trait 的完整闭集。
    let _ = MailUserCommandKind::Rekey;
}

#[test]
fn closed_season_intake_is_rejected_and_returned_instead_of_retried_forever() {
    let (decision, commands) =
        process(Equipment { invalid_source: false }, RefusingAuctions(IntakeRefusal::IntakeClosed));
    assert_eq!(decision.metadata["reason_code"], "season_intake_closed");
    assert_rejected_with(&decision, &commands, EquipmentRejectionReason::SeasonIntakeClosed);
}

#[test]
fn invalid_equipment_source_is_rejected_with_the_generic_reason() {
    let (decision, commands) = process(
        Equipment { invalid_source: true },
        RefusingAuctions(IntakeRefusal::EquipmentConflict),
    );
    assert_eq!(decision.metadata["reason_code"], "auction_equipment_rejected");
    assert_rejected_with(&decision, &commands, EquipmentRejectionReason::Unverified);
}

/// 已接受投稿的领取命令与拒收路径一样经统一投递判定入队：规范 origin_id、无金额、合法附件。
#[test]
fn accepted_submission_take_passes_the_unified_judgement() {
    let (decision, commands) = process(Equipment { invalid_source: false }, AcceptedAuctions);
    assert_eq!(decision.decision_code, "auction_accepted");
    assert_eq!(commands.len(), 1, "回执事实缺失时只安排领取");
    let take = &commands[0];
    assert_eq!(take.operation, MailDeliveryOperation::Take);
    assert_eq!(take.origin_id, "main:7000002:take");
    assert_eq!(take.attachment_ids, ["900000001"]);
    assert!(take.shape().admits());
}
