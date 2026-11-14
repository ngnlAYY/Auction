//! 应用错误的闭集安全分类；不依赖错误文本，不改变原错误语义。

use super::ApplicationError;
use crate::ports::admin::{AdminMailTransactionError, AdminRulesTransactionError};
use crate::ports::core::{CommandLedgerError, CompetitionTransactionError, RepositoryError};
use crate::ports::recovery::RecoveryCommandError;
use crate::ports::settlement::SettlementTransactionError;

impl ApplicationError {
    /// 返回不含秘密与正文的闭集安全分类，仅用于诊断。
    /// 不读取 Display、Debug 或 source，不参与重试或业务判定。
    pub fn safe_kind(&self) -> &'static str {
        match self {
            Self::CapabilityUnavailable(_) => "capability_unavailable",
            Self::ForumIdentityRejected(_) => "forum_identity_rejected",
            Self::FulfillmentEvidenceNotObserved => "evidence_not_observed",
            Self::FulfillmentDependencyUnavailable => "dependency_unavailable",
            Self::FulfillmentAccountBusy => "remote_busy",
            Self::RemoteDependency { kind, .. } => kind.label(),
            Self::FulfillmentDependencyMismatch => "dependency_mismatch",
            Self::FulfillmentEquipmentSecretNotFound => "equipment_secret_not_found",
            Self::Domain(_) => "domain_rejected",
            Self::Repository(RepositoryError::Timeout) => "timeout",
            Self::Repository(RepositoryError::Unavailable) => "dependency_unavailable",
            Self::Repository(RepositoryError::Failed(_))
            | Self::CompetitionTransaction(CompetitionTransactionError::Failed(_))
            | Self::SettlementTransaction(SettlementTransactionError::Failed(_))
            | Self::RecoveryCommand(RecoveryCommandError::Failed(_))
            | Self::AdminRulesTransaction(AdminRulesTransactionError::Failed(_))
            | Self::AdminMailTransaction(AdminMailTransactionError::Failed(_)) => {
                "repository_failed"
            }
            Self::DeliveryAdmin(_) => "delivery_admin_error",
            Self::CompetitionTransaction(CompetitionTransactionError::SerializationConflict)
            | Self::SettlementTransaction(SettlementTransactionError::SerializationConflict)
            | Self::RecoveryCommand(RecoveryCommandError::SerializationConflict)
            | Self::AdminRulesTransaction(AdminRulesTransactionError::SerializationConflict)
            | Self::AdminMailTransaction(AdminMailTransactionError::SerializationConflict) => {
                "repository_serialization_conflict"
            }
            Self::CompetitionTransaction(CompetitionTransactionError::QueueWaitTimeout) => {
                "competition_queue_timeout"
            }
            Self::CompetitionTransaction(CompetitionTransactionError::Conflict)
            | Self::SettlementTransaction(SettlementTransactionError::Conflict)
            | Self::AdminRulesTransaction(AdminRulesTransactionError::Conflict)
            | Self::AdminMailTransaction(AdminMailTransactionError::Conflict) => {
                "repository_conflict"
            }
            Self::CompetitionTransaction(
                CompetitionTransactionError::NotFound
                | CompetitionTransactionError::AdminIntakeUnavailable
                | CompetitionTransactionError::IntakeClosed
                | CompetitionTransactionError::AdminIntakeSellerBlacklisted
                | CompetitionTransactionError::AdminIntakeEquipmentConflict
                | CompetitionTransactionError::StaleState
                | CompetitionTransactionError::InvalidState
                | CompetitionTransactionError::BidderIneligible
                | CompetitionTransactionError::StaleSlowProxy,
            ) => "competition_rejected",
            Self::SettlementTransaction(SettlementTransactionError::NotFound) => {
                "settlement_not_found"
            }
            Self::RecoveryCommand(RecoveryCommandError::CommandConflict) => "command_conflict",
            Self::RecoveryCommand(
                RecoveryCommandError::InvalidCommand
                | RecoveryCommandError::NotFound
                | RecoveryCommandError::StaleState
                | RecoveryCommandError::NotConfirmable
                | RecoveryCommandError::EvidenceUnresolved
                | RecoveryCommandError::InvalidStoredResult,
            ) => "recovery_rejected",
            Self::RealtimeAdmin(_) => "realtime_admin_error",
            Self::CommandLedger(error)
            | Self::CompetitionTransaction(CompetitionTransactionError::CommandLedger(error))
            | Self::SettlementTransaction(SettlementTransactionError::CommandLedger(error)) => {
                match error {
                    CommandLedgerError::InvalidRecord => "command_ledger_invalid",
                    CommandLedgerError::Conflict => "command_conflict",
                    CommandLedgerError::Storage => "repository_failed",
                }
            }
            Self::TransactionPolicy(_) => "transaction_policy_invalid",
            Self::Serialization(_) => "serialization_failed",
            Self::Session(_) => "session_error",
            Self::AdminRules(_) => "admin_rules_error",
            Self::AdminRulesTransaction(AdminRulesTransactionError::RevisionConflict {
                ..
            }) => "rules_revision_conflict",
            Self::AdminMail(_) => "admin_mail_error",
            Self::AdminMailTransaction(AdminMailTransactionError::NotFound) => {
                "mail_poll_state_not_found"
            }
            Self::AdminMailTransaction(AdminMailTransactionError::StateConflict { .. }) => {
                "mail_state_conflict"
            }
            Self::AdminIntake(_) => "admin_intake_error",
            Self::AdminForumPreview(_) => "admin_forum_preview_error",
            Self::ForumRemote(error) => match (error.write_started(), error.retryable()) {
                (true, _) => "remote_write_started",
                (false, true) => "remote_retryable",
                (false, false) => "remote_rejected",
            },
            Self::AdminAudit(_) => "admin_audit_error",
            Self::AdminCredential(_) => "admin_credential_error",
            Self::AdminLotUpdate(_) => "admin_lot_update_error",
            Self::AdminRuntime(_) => "admin_runtime_error",
            Self::AdminRuntimeFile(_) => "runtime_file_error",
            Self::AdminRuntimeCommand(_) => "runtime_command_error",
            Self::AdminUser(_) => "admin_user_error",
            Self::AdminWorld(_) => "admin_world_error",
            Self::MailProcessing(_) => "mail_processing_error",
            Self::OperationsAlert(_) => "operations_alert_error",
            Self::PlayerNotification(_) => "player_notification_error",
            Self::DeliveryDisposition(error) => match error {
                crate::ports::delivery::DeliveryDispositionCommandError::Invalid => {
                    "delivery_disposition_invalid"
                }
                crate::ports::delivery::DeliveryDispositionCommandError::NotFound => {
                    "delivery_not_found"
                }
                crate::ports::delivery::DeliveryDispositionCommandError::Stale => "delivery_stale",
                crate::ports::delivery::DeliveryDispositionCommandError::AlreadyDisposed => {
                    "delivery_already_disposed"
                }
                crate::ports::delivery::DeliveryDispositionCommandError::NotAcknowledgeable => {
                    "delivery_not_acknowledgeable"
                }
                crate::ports::delivery::DeliveryDispositionCommandError::CommandConflict => {
                    "command_conflict"
                }
                crate::ports::delivery::DeliveryDispositionCommandError::Unavailable => {
                    "repository_failed"
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::errors::safe_kind as parent;

    #[test]
    fn remote_dependency_keeps_the_remote_classification() {
        use crate::errors::diagnostics::SafeErrorKind;
        let busy = ApplicationError::RemoteDependency {
            kind: SafeErrorKind::REMOTE_ACCOUNT_BUSY,
            detail: "Sent 列表读取失败".to_owned(),
        };
        let drift = ApplicationError::RemoteDependency {
            kind: SafeErrorKind::REMOTE_PROTOCOL_ERROR,
            detail: "Sent 列表读取失败".to_owned(),
        };
        assert_eq!(busy.safe_kind(), "remote_account_busy");
        assert_eq!(drift.safe_kind(), "remote_protocol_error");
        // Display 仍是适配器给出的脱敏说明，维护报告沿用原文案。
        assert_eq!(drift.to_string(), "Sent 列表读取失败");
    }

    #[test]
    fn readiness_errors_do_not_project_as_repository_failed() {
        let timeout = ApplicationError::from(RepositoryError::Timeout);
        let unavailable = ApplicationError::from(RepositoryError::Unavailable);
        assert_eq!(timeout.safe_kind(), "timeout");
        assert_eq!(unavailable.safe_kind(), "dependency_unavailable");
        assert_ne!(timeout.safe_kind(), "repository_failed");
        assert_ne!(unavailable.safe_kind(), "repository_failed");
    }
}
