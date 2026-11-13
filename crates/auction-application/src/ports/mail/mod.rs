//! mail 上下文。

pub(crate) mod diagnostics;
pub(crate) mod display;
pub(crate) mod equipment_diagnostics;
pub(crate) mod maintenance;
pub(crate) mod poll;
pub(crate) mod processing;
mod reply_policy;
pub(crate) mod scan;
pub(crate) mod scan_checkpoint;
pub(crate) mod scheduling;
pub(crate) mod user;

pub use diagnostics::{
    MailDiagnosticContext, MailDiagnosticError, MailDiagnosticFuture, MailDiagnosticOutcome,
    MailDiagnosticReceipt, MailDiagnosticRecorder, MailDiagnosticStage, PreparedMailEvidence,
};
pub use display::MailDisplaySnapshot;
pub use equipment_diagnostics::*;
pub use maintenance::{
    MailScanPageObservation, MailboxFuture, MoogleMailboxComposeState, MoogleMailboxDiscard,
    MoogleMailboxEvidenceStore, MoogleMailboxInboxAssets, MoogleMailboxInboxObservation,
    MoogleMailboxMaintenance, MoogleMailboxNoticeExpectation, MoogleMailboxSentEntries,
    MoogleMailboxSentEntry, MoogleMailboxSentState, MoogleMailboxSweepCandidate,
    MoogleMailboxUnprovenSend, mail_recipient_is_list_projection, sent_list_minute_start,
};
pub use poll::{
    MAX_MAIL_POLL_BATCH_SIZE, MAX_MAIL_POLL_DECISION_BYTES, MAX_MAIL_POLL_FAILURE_CODE_BYTES,
    MAX_MAIL_POLL_FAILURE_DETAIL_BYTES, MAX_MAIL_PROGRESS_POLLERS, MailPollBatch, MailPollClaim,
    MailPollCompletion, MailPollIntake, MailPollProgressFailure, MailPollProgressObserver,
    MailPollProgressOutcome, MailPollProgressSnapshot, MailPollProgressStage, MailPollReadJournal,
    MailPollReadSummary, MailPollStore, is_safe_mail_poll_id,
};
pub use processing::{
    MailAuctionAcceptance, MailAuctionIntakeCommand, MailAuctionIntakeLookup,
    MailAuctionIntakePort, MailAuctionIntakeRecord, MailAuctionIntakeResult,
    MailAuctionIntakeTransaction, MailAuctionIntakeWrite, MailAuctionItem, MailAuctionItemWrite,
    MailDeliveryActivation, MailDeliveryCommand, MailDeliveryEnqueuer, MailDeliveryOperation,
    MailDeliveryOrigin, MailDeliveryResult, MailEquipmentAttachment, MailEquipmentFilter,
    MailEquipmentReader, MailMessage, MailNoticeEnqueue, MailNoticeLimit, MailNotificationTemplate,
    MailProcessingAttempt, MailProcessingDecision, MailProcessingError, MailProcessingFuture,
    MailProcessor, SubmissionReceiptFacts, SubmissionReceiptLot, default_accepted_qualities,
    explanation_notice_limit, mail_auction_aggregate_id, valid_notification_body,
};
pub use reply_policy::{MailReplyPolicy, MailReplyPolicySource, MailSendClass, MailSendClasses};
pub use scan::{
    MAIL_SCAN_DAILY_FULL_INTERVAL, MailPrefixOutcome, MailPrefixPage, MailPrefixScan,
    MailScanAnchor, MailScanCheckpoint, MailScanEvent, MailScanMode, MailScanObserver,
    MailScanOutcome, MailScanPlan, MailScanReason, MailScanShadow, MailScanStream,
    MailScanWatermark, advance_mail_scan_watermark, compare_mail_scan_mids,
    noop_mail_scan_observer, numeric_mail_scan_id, plan_mail_scan,
};
pub use scan_checkpoint::{
    MailScanCheckpointFuture, MailScanCheckpointKey, MailScanCheckpointStore,
};
pub use scheduling::{
    GateClass, GatePoll, GateTicket, LOW_PRIORITY_FLOOR_AFTER, OperationLockState,
    PriorityGateState, SegmentPlan,
};
pub use user::{
    MailLoginKey, MailLoginKeyEnvelope, MailLoginKeyEnvelopeReader, MailLoginKeyIssue,
    MailLoginKeyIssuer, MailLoginKeyReader, MailUserCommandKind, MailUserPort,
    MailUserRebindCommand, MailUserRegisterCommand, MailUserRegisterWrite, MailUserRekeyCommand,
    MailUserRekeyWrite, MailUserResult, MailUserResultCode, MailUserStore,
};
