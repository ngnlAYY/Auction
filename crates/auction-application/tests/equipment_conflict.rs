//! 已登记装备再次投稿时必须收口为退回，而不是永久卡住邮件游标。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::future::Future;
use std::sync::{Arc, Mutex};

use auction_application::admin::AdminIntakeSourceSnapshot;
use auction_application::auction::ApplicationFuture;
use auction_application::core::{ApplicationError, Clock, CompetitionTransactionError};
use auction_application::mail::{
    MailAuctionIntakeCommand, MailAuctionIntakeLookup, MailAuctionIntakePort,
    MailAuctionIntakeResult, MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer,
    MailDeliveryResult, MailEquipmentAttachment, MailEquipmentReader, MailMessage,
    MailNoticeEnqueue, MailNoticeLimit, MailProcessor, MailProcessorConfig, MailProcessorService,
    MailUserCommandKind, MailUserPort, MailUserRebindCommand, MailUserRegisterCommand,
    MailUserRekeyCommand, MailUserResult,
};
use auction_domain::EquipmentKey;
use chrono::{DateTime, Utc};
use serde_json::json;
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

struct LoadedEquipment;

impl MailEquipmentReader for LoadedEquipment {
    fn read(
        &self,
        _world_id: String,
        _attachment: MailEquipmentAttachment,
    ) -> ApplicationFuture<'_, AdminIntakeSourceSnapshot> {
        Box::pin(async {
            Ok(AdminIntakeSourceSnapshot {
                adapter: "test".to_owned(),
                source_fingerprint: [7; 32],
                external_equipment_id: "316457702".to_owned(),
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

struct ConflictAuctions;

impl MailAuctionIntakePort for ConflictAuctions {
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
        Box::pin(async {
            Err(ApplicationError::CompetitionTransaction(
                CompetitionTransactionError::AdminIntakeEquipmentConflict,
            ))
        })
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

#[test]
fn duplicate_equipment_is_rejected_and_returned_instead_of_retried_forever() {
    let deliveries = Arc::new(RecordingDeliveries::default());
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
        Arc::new(LoadedEquipment),
        Arc::new(ConflictAuctions),
        Arc::clone(&deliveries),
        Arc::new(NoUsers),
        Arc::new(FixedClock),
    )
    .expect("processor configuration");
    let message = MailMessage {
        mailbox_key: "mooglemail:main".to_owned(),
        external_mail_id: "3126617".to_owned(),
        world_id: "main".to_owned(),
        received_at: at(NOW),
        sender: "armande1".to_owned(),
        subject: "auction".to_owned(),
        body: "========== Attachment ==========\n\n[316457702] Legendary Ethereal Great Mace of the Vampire\n\n================================".to_owned(),
        credits: 0,
        cod: 0,
        attachments: vec![MailEquipmentAttachment {
            external_equipment_id: "316457702".to_owned(),
            key: EquipmentKey::new("34c20d12dc").unwrap(),
        }],
        returnable: true,
        claimable: true,
        content_digest: [9; 32],
    };
    let decision =
        block_on(processor.process(message)).result.expect("must be rejected, not error");
    assert_eq!(decision.status, "rejected");
    assert_eq!(decision.decision_code, "auction_equipment_rejected");
    assert_eq!(decision.metadata["reason_code"], "auction_equipment_conflict");
    let commands = deliveries.commands.lock().unwrap();
    assert_eq!(commands.len(), 2, "必须同时安排退回和说明信");
    assert!(commands.iter().any(|command| {
        command.operation == auction_application::mail::MailDeliveryOperation::Return
    }));
    assert!(commands.iter().any(|command| {
        command.template
            == Some(auction_application::mail::MailNotificationTemplate::AuctionEquipmentRejected)
    }));
    // 用户状态/命令枚举只用于确保测试装配满足 trait 的完整闭集。
    let _ = MailUserCommandKind::Rekey;
}
