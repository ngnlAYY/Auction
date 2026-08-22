mod sessions;

use thiserror::Error;

use auction_domain::DomainError;

use crate::{
    AdminAuditError, AdminForumPreviewError, AdminIntakeError, AdminLotUpdateError, AdminMailError,
    AdminMailTransactionError, AdminRulesError, AdminRulesTransactionError,
    AdminRuntimeCommandError, AdminRuntimeConfigError, AdminRuntimeFileError, CommandLedgerError,
    CompetitionTransactionError, RealtimeAdminError, RecoveryCommandError, RepositoryError,
    SettlementTransactionError, SettlementTransactionFailure, TransactionFailure,
    TransactionPolicyError,
};
use serde_json::Error as JsonError;

pub use sessions::SessionError;

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("capability unavailable: {0}")]
    CapabilityUnavailable(&'static str),

    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error(transparent)]
    Repository(#[from] RepositoryError),

    #[error(transparent)]
    CompetitionTransaction(#[from] CompetitionTransactionError),

    #[error(transparent)]
    SettlementTransaction(#[from] SettlementTransactionError),

    #[error(transparent)]
    RecoveryCommand(#[from] RecoveryCommandError),

    #[error(transparent)]
    RealtimeAdmin(#[from] RealtimeAdminError),

    #[error(transparent)]
    CommandLedger(#[from] CommandLedgerError),

    #[error(transparent)]
    TransactionPolicy(#[from] TransactionPolicyError),

    #[error(transparent)]
    Serialization(#[from] JsonError),

    #[error(transparent)]
    Session(#[from] SessionError),

    #[error(transparent)]
    AdminRules(#[from] AdminRulesError),

    #[error(transparent)]
    AdminRulesTransaction(#[from] AdminRulesTransactionError),

    #[error(transparent)]
    AdminMail(#[from] AdminMailError),

    #[error(transparent)]
    AdminMailTransaction(#[from] AdminMailTransactionError),

    #[error(transparent)]
    AdminIntake(#[from] AdminIntakeError),

    #[error(transparent)]
    AdminForumPreview(#[from] AdminForumPreviewError),

    #[error(transparent)]
    AdminAudit(#[from] AdminAuditError),

    #[error(transparent)]
    AdminLotUpdate(#[from] AdminLotUpdateError),

    #[error(transparent)]
    AdminRuntime(#[from] AdminRuntimeConfigError),

    #[error(transparent)]
    AdminRuntimeFile(#[from] AdminRuntimeFileError),

    #[error(transparent)]
    AdminRuntimeCommand(#[from] AdminRuntimeCommandError),
}

impl TransactionFailure for ApplicationError {
    fn is_serialization_conflict(&self) -> bool {
        matches!(
            self,
            Self::CompetitionTransaction(CompetitionTransactionError::SerializationConflict)
                | Self::SettlementTransaction(SettlementTransactionError::SerializationConflict)
                | Self::AdminRulesTransaction(AdminRulesTransactionError::SerializationConflict)
                | Self::AdminMailTransaction(AdminMailTransactionError::SerializationConflict)
        )
    }
}

impl SettlementTransactionFailure for ApplicationError {
    fn is_serialization_conflict(&self) -> bool {
        matches!(
            self,
            Self::SettlementTransaction(SettlementTransactionError::SerializationConflict)
        )
    }
}

impl crate::AdminRulesTransactionFailure for ApplicationError {
    fn is_serialization_conflict(&self) -> bool {
        matches!(
            self,
            Self::AdminRulesTransaction(AdminRulesTransactionError::SerializationConflict)
        )
    }
}

impl crate::AdminMailTransactionFailure for ApplicationError {
    fn is_serialization_conflict(&self) -> bool {
        matches!(self, Self::AdminMailTransaction(AdminMailTransactionError::SerializationConflict))
    }
}
