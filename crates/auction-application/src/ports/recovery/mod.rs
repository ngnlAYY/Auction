//! recovery 上下文。

pub(crate) mod commands;
pub(crate) mod external;
pub(crate) mod health;
pub(crate) mod report;
pub(crate) mod scheduler;

pub use commands::{
    RecoveryCommandError, RecoveryCommandPort, RecoveryCommandRepository, RecoveryCommandService,
    RecoveryConfirmationAction, RecoveryConfirmationCommand, RecoveryConfirmationResult,
};
pub use external::{
    EXTERNAL_RECOVERY_RECEIPT_FORMAT, ExternalRecoveryDeliveryExpectation,
    ExternalRecoveryForumThread, ExternalRecoveryFuture, ExternalRecoveryQuarantineMatch,
    ExternalRecoveryQuarantineOutcome, ExternalRecoveryStore, MAX_EXTERNAL_RECOVERY_DELIVERIES,
    MAX_EXTERNAL_RECOVERY_FORUM_THREADS, MAX_EXTERNAL_RECOVERY_SENT_MATCHES, SentDetailMatch,
    sent_detail_match, sent_list_projection_may_match, subject_is_list_projection,
};
pub use health::{
    RecoveryBalanceStatus, RecoveryEvidenceStatus, RecoveryHealthConfig, RecoveryHealthRefreshPort,
    RecoveryHealthStore, RecoveryHealthUpdate,
};
pub use report::{
    RecoveryActionReport, RecoveryReport, RecoveryReportEntry, RecoveryReportReader,
    allowed_recovery_commands,
};
pub use scheduler::{
    RecoveryActionClaim, RecoveryActionOutcome, RecoveryCaseAdvance, RecoveryResolutionAdvance,
    RecoveryResolutionStore, RecoveryReturnAdvance, RecoveryWorkStore,
};
