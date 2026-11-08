//! recovery 上下文公开 API。

pub use crate::ports::recovery::{
    EXTERNAL_RECOVERY_AMBIGUOUS_REASON, EXTERNAL_RECOVERY_RECEIPT_FORMAT,
    ExternalRecoveryDeliveryExpectation, ExternalRecoveryForumThread, ExternalRecoveryFuture,
    ExternalRecoveryQuarantineMatch, ExternalRecoveryQuarantineOutcome,
    ExternalRecoverySendEvidence, ExternalRecoveryStore, ExternalRecoveryUnresolvedBlock,
    MAX_EXTERNAL_RECOVERY_BLOCK_MAIL_IDS, MAX_EXTERNAL_RECOVERY_DELIVERIES,
    MAX_EXTERNAL_RECOVERY_FORUM_THREADS, MAX_EXTERNAL_RECOVERY_SENT_MATCHES, RecoveryActionClaim,
    RecoveryActionOutcome, RecoveryActionReport, RecoveryBalanceStatus, RecoveryCaseAdvance,
    RecoveryCommandError, RecoveryCommandPort, RecoveryCommandRepository,
    RecoveryConfirmationAction, RecoveryConfirmationCommand, RecoveryConfirmationResult,
    RecoveryEvidenceStatus, RecoveryHealthConfig, RecoveryHealthRefreshPort, RecoveryHealthStore,
    RecoveryHealthUpdate, RecoveryReport, RecoveryReportEntry, RecoveryReportReader,
    RecoveryResolutionAdvance, RecoveryResolutionStore, RecoveryReturnAdvance, RecoveryWorkStore,
    SentDetailMatch, allowed_recovery_commands, sent_detail_match, sent_list_projection_may_match,
    subject_is_list_projection,
};
pub use crate::services::recovery::{RecoveryCommandService, RecoveryHealthService};
