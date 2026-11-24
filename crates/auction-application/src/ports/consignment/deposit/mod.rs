//! 寄售第 4 期收件与代管的窄端口、命令、回执与拒绝类型。

mod commands;
mod control;
mod disposition;
mod envelope;
mod owner;
mod poll;
mod readiness;
mod reads;
mod reservation;
mod season;
mod season_task;
mod secrets;

pub use commands::{
    DEPOSIT_SCHEMA_VERSION, DepositActor, DepositOperation, ListingStateChange, ReceiptDisposition,
};
pub use control::{
    CustodyControlStore, OperationsHoldDisposition, OperationsHoldReceipt,
    ReleaseOperationsHoldCommand, SetOperationsHoldCommand, WithdrawDepositCommand,
    WithdrawDepositReceipt, WithdrawDisposition,
};
pub use disposition::{
    ApplyMailProofCommand, CustodyListingChange, DispositionAction, DispositionStore,
    MailProofKind, PostPermit, ProofDisposition, ProofReceipt, StartDispositionWriteCommand,
    StartReceipt, WriteStart, WriteStartOutcome,
};
pub use envelope::{
    AppendQuarantineObservationCommand, AutoReleaseBinding, DecideEnvelopeCommand,
    EnvelopeDisposition, EnvelopeReceipt, EnvelopeStore, ObservationReceipt, QuarantineReference,
    RecheckQuarantineCommand, RecheckReceipt, ReleaseQuarantineCommand, ReleaseReceipt,
};
pub use owner::LegacyAuctionOwnerResolver;
pub use poll::{
    EnvelopePreflight, MailReadinessGate, PollEnvelopeStore, PollEnvelopeWrite, PollRawObservation,
};
pub use readiness::{
    MailEvidenceReadinessRecord, MailEvidenceReadinessStore, MailEvidenceSessionDraft,
    MailReadinessPolicy, MailReadinessReceipt,
};
pub use reads::{
    ActiveDepositInstructions, AdminDepositAction, AdminDepositDetail, AdminDepositQuery,
    AdminFinalDecision, AdminObservationItem, AdminQuarantineObservation, DepositRead,
    DepositStatus, DepositWorldStatus, DepositWorlds, OwnedDepositInstructions, QuarantineState,
};
pub use reservation::{
    ExpireReservationCommand, IssueReservationCommand, ReservationInvalidation, ReservationReceipt,
    ReservationRetired, ReservationStore,
};
pub use season::{
    SeasonCloseCommand, SeasonCloseReceipt, SeasonCloseTarget, SeasonNoticeCommand,
    SeasonNoticeKind, SeasonNoticeReceipt, SeasonStore,
};
pub use season_task::{
    CONSIGNMENT_SEASON_NOTICE_RESULT_CODE, CONSIGNMENT_SEASON_NOTICE_SUBJECT, SeasonNoticeDelivery,
    SeasonNoticeDeliveryCommand, SeasonNoticeTarget, SeasonTaskStore, season_notice_body,
    season_notice_delivery_command, season_notice_delivery_id, season_notice_external_mail_id,
    season_notice_origin_id, season_notice_work_id,
};
pub use secrets::{
    ReservationSecretAad, ReservationSecretCrypto, ReservationSecretCryptoError,
    SealedReservationSecret,
};

#[cfg(test)]
pub(crate) mod test_support;
