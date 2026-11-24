//! 应用错误的闭集安全分类；不依赖错误文本，不改变原错误语义。

use super::ApplicationError;
use crate::errors::diagnostics::{DiagnosticError, SafeErrorKind};
use crate::ports::admin::{AdminMailTransactionError, AdminRulesTransactionError};
use crate::ports::core::{CommandLedgerError, CompetitionTransactionError, RepositoryError};
use crate::ports::delivery::DeliveryDispositionCommandError;
use crate::ports::recovery::RecoveryCommandError;
use crate::ports::settlement::{SettlementReturnError, SettlementTransactionError};

impl ApplicationError {
    /// 返回不含秘密与正文的闭集安全分类标签，仅用于诊断；与诊断投影同源，不另建字符串表。
    /// 不读取 Display、Debug 或 source，不参与重试或业务判定。
    pub fn safe_kind(&self) -> &'static str {
        self.diagnostic_kind().label()
    }
}

/// 类型化变体直接投影为闭集常量；新增变体必须在这里选定类别，不经字符串往返落入 other。
impl DiagnosticError for ApplicationError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        match self {
            Self::CapabilityUnavailable(_) => SafeErrorKind::CAPABILITY_UNAVAILABLE,
            Self::ForumIdentityRejected(_) => SafeErrorKind::FORUM_IDENTITY_REJECTED,
            Self::FulfillmentEvidenceNotObserved => SafeErrorKind::EVIDENCE_NOT_OBSERVED,
            Self::FulfillmentDependencyUnavailable => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::FulfillmentAccountBusy => SafeErrorKind::REMOTE_BUSY,
            Self::RemoteDependency { kind, .. } => *kind,
            Self::FulfillmentDependencyMismatch => SafeErrorKind::DEPENDENCY_MISMATCH,
            Self::FulfillmentEquipmentSecretNotFound => SafeErrorKind::EQUIPMENT_SECRET_NOT_FOUND,
            Self::Domain(_) => SafeErrorKind::DOMAIN_REJECTED,
            Self::Repository(RepositoryError::Timeout) => SafeErrorKind::TIMEOUT,
            Self::Repository(RepositoryError::Unavailable) => SafeErrorKind::DEPENDENCY_UNAVAILABLE,
            Self::Repository(RepositoryError::Failed(_))
            | Self::CompetitionTransaction(CompetitionTransactionError::Failed(_))
            | Self::SettlementTransaction(SettlementTransactionError::Failed(_))
            | Self::RecoveryCommand(RecoveryCommandError::Failed(_))
            | Self::AdminRulesTransaction(AdminRulesTransactionError::Failed(_))
            | Self::AdminMailTransaction(AdminMailTransactionError::Failed(_)) => {
                SafeErrorKind::REPOSITORY_FAILED
            }
            Self::DeliveryAdmin(_) => SafeErrorKind::DELIVERY_ADMIN_ERROR,
            Self::CompetitionTransaction(CompetitionTransactionError::SerializationConflict)
            | Self::SettlementTransaction(SettlementTransactionError::SerializationConflict)
            | Self::RecoveryCommand(RecoveryCommandError::SerializationConflict)
            | Self::AdminRulesTransaction(AdminRulesTransactionError::SerializationConflict)
            | Self::AdminMailTransaction(AdminMailTransactionError::SerializationConflict) => {
                SafeErrorKind::REPOSITORY_SERIALIZATION_CONFLICT
            }
            Self::CompetitionTransaction(CompetitionTransactionError::QueueWaitTimeout) => {
                SafeErrorKind::COMPETITION_QUEUE_TIMEOUT
            }
            // 与出价排队同义：实物锁在预算内未取得，不重试事务，客户端可原样重试。
            Self::CompetitionTransaction(CompetitionTransactionError::EquipmentLockUnavailable) => {
                SafeErrorKind::COMPETITION_QUEUE_TIMEOUT
            }
            Self::CompetitionTransaction(CompetitionTransactionError::Conflict)
            | Self::SettlementTransaction(SettlementTransactionError::Conflict)
            | Self::AdminRulesTransaction(AdminRulesTransactionError::Conflict)
            | Self::AdminMailTransaction(AdminMailTransactionError::Conflict) => {
                SafeErrorKind::REPOSITORY_CONFLICT
            }
            Self::CompetitionTransaction(
                CompetitionTransactionError::NotFound
                | CompetitionTransactionError::AdminIntakeUnavailable
                | CompetitionTransactionError::IntakeClosed
                | CompetitionTransactionError::AdminIntakeSellerBlacklisted
                | CompetitionTransactionError::AdminIntakeEquipmentConflict
                | CompetitionTransactionError::EquipmentHeld
                | CompetitionTransactionError::StaleState
                | CompetitionTransactionError::InvalidState
                | CompetitionTransactionError::BidderIneligible
                | CompetitionTransactionError::StaleSlowProxy,
            ) => SafeErrorKind::COMPETITION_REJECTED,
            Self::SettlementTransaction(SettlementTransactionError::NotFound) => {
                SafeErrorKind::SETTLEMENT_NOT_FOUND
            }
            Self::RecoveryCommand(RecoveryCommandError::CommandConflict) => {
                SafeErrorKind::COMMAND_CONFLICT
            }
            Self::RecoveryCommand(
                RecoveryCommandError::InvalidCommand
                | RecoveryCommandError::NotFound
                | RecoveryCommandError::StaleState
                | RecoveryCommandError::NotConfirmable
                | RecoveryCommandError::EvidenceUnresolved
                | RecoveryCommandError::SendUnattributed
                | RecoveryCommandError::InvalidStoredResult,
            ) => SafeErrorKind::RECOVERY_REJECTED,
            Self::RealtimeAdmin(_) => SafeErrorKind::REALTIME_ADMIN_ERROR,
            Self::CommandLedger(error)
            | Self::CompetitionTransaction(CompetitionTransactionError::CommandLedger(error))
            | Self::SettlementTransaction(SettlementTransactionError::CommandLedger(error)) => {
                match error {
                    CommandLedgerError::InvalidRecord => SafeErrorKind::COMMAND_LEDGER_INVALID,
                    CommandLedgerError::Conflict => SafeErrorKind::COMMAND_CONFLICT,
                    CommandLedgerError::Storage => SafeErrorKind::REPOSITORY_FAILED,
                }
            }
            Self::TransactionPolicy(_) => SafeErrorKind::TRANSACTION_POLICY_INVALID,
            Self::Serialization(_) => SafeErrorKind::SERIALIZATION_FAILED,
            Self::Session(_) => SafeErrorKind::SESSION_ERROR,
            Self::AdminRules(_) => SafeErrorKind::ADMIN_RULES_ERROR,
            Self::AdminRulesTransaction(AdminRulesTransactionError::RevisionConflict {
                ..
            }) => SafeErrorKind::RULES_REVISION_CONFLICT,
            Self::AdminMail(_) => SafeErrorKind::ADMIN_MAIL_ERROR,
            Self::AdminMailTransaction(AdminMailTransactionError::NotFound) => {
                SafeErrorKind::MAIL_POLL_STATE_NOT_FOUND
            }
            Self::AdminMailTransaction(AdminMailTransactionError::StateConflict { .. }) => {
                SafeErrorKind::MAIL_STATE_CONFLICT
            }
            Self::AdminIntake(_) => SafeErrorKind::ADMIN_INTAKE_ERROR,
            Self::AdminForumPreview(_) => SafeErrorKind::ADMIN_FORUM_PREVIEW_ERROR,
            Self::ForumRemote(error) => match (error.write_started(), error.retryable()) {
                (true, _) => SafeErrorKind::REMOTE_WRITE_STARTED,
                (false, true) => SafeErrorKind::REMOTE_RETRYABLE,
                (false, false) => SafeErrorKind::REMOTE_REJECTED,
            },
            Self::AdminAudit(_) => SafeErrorKind::ADMIN_AUDIT_ERROR,
            Self::AdminCredential(_) => SafeErrorKind::ADMIN_CREDENTIAL_ERROR,
            Self::AdminLotUpdate(_) => SafeErrorKind::ADMIN_LOT_UPDATE_ERROR,
            Self::AdminRuntime(_) => SafeErrorKind::ADMIN_RUNTIME_ERROR,
            Self::AdminRuntimeFile(_) => SafeErrorKind::RUNTIME_FILE_ERROR,
            Self::AdminRuntimeCommand(_) => SafeErrorKind::RUNTIME_COMMAND_ERROR,
            Self::AdminUser(_) => SafeErrorKind::ADMIN_USER_ERROR,
            Self::AdminWorld(_) => SafeErrorKind::ADMIN_WORLD_ERROR,
            Self::MailProcessing(_) => SafeErrorKind::MAIL_PROCESSING_ERROR,
            Self::OperationsAlert(_) => SafeErrorKind::OPERATIONS_ALERT_ERROR,
            Self::PlayerNotification(_) => SafeErrorKind::PLAYER_NOTIFICATION_ERROR,
            Self::DeliveryDisposition(error) => match error {
                DeliveryDispositionCommandError::Invalid => {
                    SafeErrorKind::DELIVERY_DISPOSITION_INVALID
                }
                DeliveryDispositionCommandError::NotFound => SafeErrorKind::DELIVERY_NOT_FOUND,
                DeliveryDispositionCommandError::Stale => SafeErrorKind::DELIVERY_STALE,
                DeliveryDispositionCommandError::AlreadyDisposed => {
                    SafeErrorKind::DELIVERY_ALREADY_DISPOSED
                }
                DeliveryDispositionCommandError::NotAcknowledgeable => {
                    SafeErrorKind::DELIVERY_NOT_ACKNOWLEDGEABLE
                }
                DeliveryDispositionCommandError::CommandConflict => SafeErrorKind::COMMAND_CONFLICT,
                DeliveryDispositionCommandError::Unavailable => SafeErrorKind::REPOSITORY_FAILED,
            },
            Self::SettlementReturn(error) => match error {
                SettlementReturnError::CommandConflict => SafeErrorKind::COMMAND_CONFLICT,
                SettlementReturnError::StoredResultCorrupt | SettlementReturnError::Unavailable => {
                    SafeErrorKind::REPOSITORY_FAILED
                }
                _ => SafeErrorKind::SETTLEMENT_RETURN_REJECTED,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApplicationError, CompetitionTransactionError, DeliveryDispositionCommandError,
        DiagnosticError, RepositoryError, SafeErrorKind,
    };

    #[test]
    fn remote_dependency_keeps_the_remote_classification() {
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

    #[test]
    fn previously_unregistered_labels_keep_their_own_diagnostic_kind() {
        use crate::ports::player_notifications::PlayerNotificationError;

        // 这些变体必须投影为各自的常量，不得落入 other；日志标签保持原值。
        let cases = [
            (
                ApplicationError::from(CompetitionTransactionError::QueueWaitTimeout),
                SafeErrorKind::COMPETITION_QUEUE_TIMEOUT,
                "competition_queue_timeout",
            ),
            (
                ApplicationError::from(CompetitionTransactionError::EquipmentLockUnavailable),
                SafeErrorKind::COMPETITION_QUEUE_TIMEOUT,
                "competition_queue_timeout",
            ),
            (
                ApplicationError::from(CompetitionTransactionError::EquipmentHeld),
                SafeErrorKind::COMPETITION_REJECTED,
                "competition_rejected",
            ),
            (
                ApplicationError::from(PlayerNotificationError::Unavailable),
                SafeErrorKind::PLAYER_NOTIFICATION_ERROR,
                "player_notification_error",
            ),
            (
                ApplicationError::from(DeliveryDispositionCommandError::Invalid),
                SafeErrorKind::DELIVERY_DISPOSITION_INVALID,
                "delivery_disposition_invalid",
            ),
            (
                ApplicationError::from(DeliveryDispositionCommandError::NotFound),
                SafeErrorKind::DELIVERY_NOT_FOUND,
                "delivery_not_found",
            ),
            (
                ApplicationError::from(DeliveryDispositionCommandError::Stale),
                SafeErrorKind::DELIVERY_STALE,
                "delivery_stale",
            ),
            (
                ApplicationError::from(DeliveryDispositionCommandError::AlreadyDisposed),
                SafeErrorKind::DELIVERY_ALREADY_DISPOSED,
                "delivery_already_disposed",
            ),
            (
                ApplicationError::from(DeliveryDispositionCommandError::NotAcknowledgeable),
                SafeErrorKind::DELIVERY_NOT_ACKNOWLEDGEABLE,
                "delivery_not_acknowledgeable",
            ),
        ];
        for (error, kind, label) in cases {
            assert_eq!(error.diagnostic_kind(), kind, "{label}");
            assert_eq!(error.safe_kind(), label);
            assert_ne!(error.diagnostic_kind(), SafeErrorKind::OTHER, "{label}");
        }
    }
}
