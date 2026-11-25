//! mail 上下文。

pub(crate) mod maintenance;
pub(crate) mod processing;
pub(crate) mod user;

pub use maintenance::{
    ComposeCleanupFailure, ConsignmentDispositionExecution, ConsignmentDispositionProof,
    ConsignmentDispositionRequest, ConsignmentDispositionTransport, ConsignmentNotSent,
    ConsignmentNotSentReason, ConsignmentSentUnprovenReason, DispositionMaintenanceService,
    DispositionManifest, DispositionManifestAttachment, DispositionObservationEvidence,
    DispositionReconcileReason, DispositionRetryReason, DispositionRound, DispositionRoundRequest,
    DispositionWorkClaim, DispositionWorkStore, MailboxMaintenanceReport,
    MailboxMaintenanceService, PreparedDisposition, QuarantineMailLocator, QuarantineRecheckClaim,
    QuarantineRecheckReadError, QuarantineRecheckReader, QuarantineRecheckRound,
    QuarantineRecheckService, QuarantineRecheckStore, QuarantineReobservation, ReturnTakeClaim,
    ReturnTakeLease, ReturnTakeWorkStore, SignedConsignmentProof, disposition_proof_observation_id,
};
pub use processing::{
    ConsignmentIngressOutcome, ConsignmentIngressRequest, ConsignmentIngressService,
    ConsignmentMailCandidate, ConsignmentMailInput, ConsignmentMailOutcome, ConsignmentMailPlan,
    ConsignmentMailService, ConsignmentNotApplicable, MAX_SUBMISSION_RECEIPT_BODY_BYTES,
    MailAuctionIntakeService, MailProcessorConfig, MailProcessorService,
    SUBMISSION_RECEIPT_SUBJECT, is_consignment_candidate, submission_receipt_body,
    submission_receipt_invalid_reason,
};
pub use user::MailUserService;
