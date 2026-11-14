//! 应用层错误：稳定分类、安全诊断与会话错误。

pub mod diagnostics;
mod safe_kind;
mod sessions;

use thiserror::Error;

use auction_domain::DomainError;

use crate::ports::admin::{
    AdminAuditError, AdminCredentialError, AdminForumPreviewError, AdminIntakeError,
    AdminLotUpdateError, AdminMailError, AdminMailTransactionError, AdminRulesError,
    AdminRulesTransactionError, AdminRuntimeCommandError, AdminRuntimeConfigError,
    AdminRuntimeFileError, AdminUserError, AdminWorldError,
};
use crate::ports::core::{
    CommandLedgerError, CompetitionTransactionError, RepositoryError, TransactionFailure,
    TransactionPolicyError,
};
use crate::ports::delivery::{DeliveryAdminError, DeliveryDispositionCommandError};
use crate::ports::forum::ForumRemoteError;
use crate::ports::mail::MailProcessingError;
use crate::ports::operations::OperationsAlertError;
use crate::ports::realtime::RealtimeAdminError;
use crate::ports::recovery::RecoveryCommandError;
use crate::ports::settlement::{SettlementTransactionError, SettlementTransactionFailure};
use serde_json::Error as JsonError;

pub use sessions::SessionError;

/// MoogleMail 业务处理错误是否适合在同一轮内联重试。
/// 旧版只跳过明确标记为永久失败的错误；无法识别的依赖错误仍保守重试，避免短暂存储故障
/// 被误判为坏邮件并永久阻塞游标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailProcessingRetryability {
    Permanent,
    Retryable,
}

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("capability unavailable: {0}")]
    CapabilityUnavailable(&'static str),

    /// 论坛身份已被权威事实明确拒绝；数据库故障不得使用此类别跳过较早楼层。
    #[error("论坛竞价身份不可用: {0}")]
    ForumIdentityRejected(&'static str),

    /// 外部回执读取已完成，但在受限观察窗口内没有发现指定远端对象。
    #[error("履约外部证明尚未观察到")]
    FulfillmentEvidenceNotObserved,

    /// 履约依赖当前不可读取；调用方必须进入持久退避，不能把它解释为事实不匹配。
    #[error("履约依赖暂时不可用")]
    FulfillmentDependencyUnavailable,

    /// MoogleMail 账号正在战斗，Item Shop 暂不可读；调用方应保留凭据并稍后重试。
    #[error("履约账号正在战斗，外部余额暂不可读")]
    FulfillmentAccountBusy,

    /// 外部只读依赖（如 MoogleMail 页面读取）失败。只携带闭集安全分类与适配器给出的脱敏说明，
    /// 让诊断区分账号忙、协议漂移、会话失效等远端状态与本地存储故障；不携带响应正文。
    /// 调用方的重试语义与此前的依赖失败相同，分类不参与业务判定。
    #[error("{detail}")]
    RemoteDependency { kind: diagnostics::SafeErrorKind, detail: String },

    /// 履约依赖返回了与冻结 owner 身份冲突的事实；调用方必须停止自动推进。
    #[error("履约依赖身份或事实不匹配")]
    FulfillmentDependencyMismatch,

    /// delivery owner 引用的装备 secret owner 行不存在；发送前解析必须形成可审计的明确失败。
    #[error("履约装备 secret 引用不存在")]
    FulfillmentEquipmentSecretNotFound,

    #[error(transparent)]
    Domain(#[from] DomainError),

    #[error(transparent)]
    Repository(#[from] RepositoryError),

    #[error(transparent)]
    DeliveryAdmin(#[from] DeliveryAdminError),

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
    ForumRemote(#[from] ForumRemoteError),

    #[error(transparent)]
    AdminAudit(#[from] AdminAuditError),

    #[error(transparent)]
    AdminCredential(#[from] AdminCredentialError),

    #[error(transparent)]
    AdminLotUpdate(#[from] AdminLotUpdateError),

    #[error(transparent)]
    AdminRuntime(#[from] AdminRuntimeConfigError),

    #[error(transparent)]
    AdminRuntimeFile(#[from] AdminRuntimeFileError),

    #[error(transparent)]
    AdminRuntimeCommand(#[from] AdminRuntimeCommandError),

    #[error(transparent)]
    AdminUser(#[from] AdminUserError),

    #[error(transparent)]
    AdminWorld(#[from] AdminWorldError),

    #[error(transparent)]
    MailProcessing(#[from] MailProcessingError),

    #[error(transparent)]
    OperationsAlert(#[from] OperationsAlertError),

    #[error(transparent)]
    PlayerNotification(#[from] crate::ports::player_notifications::PlayerNotificationError),

    #[error(transparent)]
    DeliveryDisposition(#[from] DeliveryDispositionCommandError),
}

impl ApplicationError {
    /// 返回 MoogleMail 单封业务处理的内联重试分类。
    pub fn mail_processing_retryability(&self) -> MailProcessingRetryability {
        use MailProcessingRetryability::{Permanent, Retryable};

        match self {
            Self::CapabilityUnavailable(_)
            | Self::Domain(_)
            | Self::Serialization(_)
            | Self::TransactionPolicy(_)
            | Self::MailProcessing(_) => Permanent,
            Self::AdminIntake(AdminIntakeError::SourceUnavailable) => Retryable,
            Self::AdminIntake(_) => Permanent,
            Self::CommandLedger(CommandLedgerError::Storage) => Retryable,
            Self::CommandLedger(_) => Permanent,
            Self::CompetitionTransaction(error) => match error {
                CompetitionTransactionError::Failed(_)
                | CompetitionTransactionError::SerializationConflict
                | CompetitionTransactionError::QueueWaitTimeout
                | CompetitionTransactionError::CommandLedger(CommandLedgerError::Storage) => {
                    Retryable
                }
                CompetitionTransactionError::NotFound
                | CompetitionTransactionError::Conflict
                | CompetitionTransactionError::AdminIntakeUnavailable
                | CompetitionTransactionError::IntakeClosed
                | CompetitionTransactionError::AdminIntakeSellerBlacklisted
                | CompetitionTransactionError::AdminIntakeEquipmentConflict
                | CompetitionTransactionError::StaleState
                | CompetitionTransactionError::InvalidState
                | CompetitionTransactionError::BidderIneligible
                | CompetitionTransactionError::StaleSlowProxy
                | CompetitionTransactionError::CommandLedger(_) => Permanent,
            },
            Self::Repository(_) => Retryable,
            // 业务处理器新增依赖错误时，沿用旧版对“未实现 Temporary 分类”的保守重试语义。
            _ => Retryable,
        }
    }
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

impl crate::ports::admin::AdminRulesTransactionFailure for ApplicationError {
    fn is_serialization_conflict(&self) -> bool {
        matches!(
            self,
            Self::AdminRulesTransaction(AdminRulesTransactionError::SerializationConflict)
        )
    }
}

impl crate::ports::admin::AdminMailTransactionFailure for ApplicationError {
    fn is_serialization_conflict(&self) -> bool {
        matches!(self, Self::AdminMailTransaction(AdminMailTransactionError::SerializationConflict))
    }
}
