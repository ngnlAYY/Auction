//! Worker 调度和各业务循环共享的安全错误分类；底层数据库、凭据和外部响应正文不穿透到控制面。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticError, SafeErrorKind};
use thiserror::Error;

/// Worker 调度器错误；数据库或应用错误只通过安全的应用错误分类向上返回。
#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("worker auction loop configuration is invalid")]
    InvalidAuctionConfiguration,
    #[error("auction worker task failed")]
    AuctionTaskJoin(#[source] tokio::task::JoinError),
    #[error("auction worker task exited unexpectedly")]
    AuctionTaskExited,
    #[error("settlement worker task failed")]
    SettlementTaskJoin(#[source] tokio::task::JoinError),
    #[error("settlement worker task exited unexpectedly")]
    SettlementTaskExited,
    #[error("worker delivery loop configuration is invalid")]
    InvalidDeliveryConfiguration,
    #[error("worker fulfillment loop configuration is invalid")]
    InvalidFulfillmentConfiguration,
    #[error("fulfillment worker task failed")]
    FulfillmentTaskJoin(#[source] tokio::task::JoinError),
    #[error("fulfillment worker task exited unexpectedly")]
    FulfillmentTaskExited,
    #[error("worker recovery loop configuration is invalid")]
    InvalidRecoveryConfiguration,
    #[error("recovery worker task failed")]
    RecoveryTaskJoin(#[source] tokio::task::JoinError),
    #[error("recovery worker task exited unexpectedly")]
    RecoveryTaskExited,
    #[error("delivery worker cycle task failed")]
    DeliveryTaskJoin(#[source] tokio::task::JoinError),
    #[error("operations alert worker task failed")]
    OperationsAlertTaskJoin(#[source] tokio::task::JoinError),
    #[error("operations alert worker task exited unexpectedly")]
    OperationsAlertTaskExited,
    #[error("player notification worker task failed")]
    PlayerNotificationTaskJoin(#[source] tokio::task::JoinError),
    #[error("player notification worker task exited unexpectedly")]
    PlayerNotificationTaskExited,
    #[error("worker identity must not be empty")]
    InvalidWorkerIdentity,
    #[error("MoogleMail poll configuration is invalid")]
    InvalidMailPollConfiguration,
    #[error("MoogleMail poll worker task failed")]
    MailPollTaskJoin(#[source] tokio::task::JoinError),
    #[error("MoogleMail poll worker task exited unexpectedly")]
    MailPollTaskExited,
    /// 独立维护任务异常触发统一 drain，未知外写仍由原持久围栏恢复。
    #[error("MoogleMail 维护任务执行失败")]
    MailMaintenanceTaskJoin(#[source] tokio::task::JoinError),
    /// 常驻维护任务在未进入 drain 时退出，不能静默失去维护能力。
    #[error("MoogleMail 维护任务意外退出")]
    MailMaintenanceTaskExited,
    #[error("forum sync configuration is invalid")]
    InvalidForumSyncConfiguration,
    #[error("forum reply configuration is invalid")]
    InvalidForumReplyConfiguration,
    #[error("forum worker loop configuration is invalid")]
    InvalidForumConfiguration,
    #[error("forum topic processing failed after the failure state was persisted")]
    ForumTopicProcessingFailed,
    #[error("forum worker task failed")]
    ForumTaskJoin(#[source] tokio::task::JoinError),
    #[error("forum worker task exited unexpectedly")]
    ForumTaskExited,
    #[error("{group} worker task failed while draining")]
    DrainTaskJoin {
        group: &'static str,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error(transparent)]
    Application(#[from] ApplicationError),
}

impl DiagnosticError for WorkerError {
    fn diagnostic_kind(&self) -> SafeErrorKind {
        use SafeErrorKind;
        match self {
            Self::Application(error) => error.diagnostic_kind(),
            Self::InvalidAuctionConfiguration
            | Self::InvalidDeliveryConfiguration
            | Self::InvalidFulfillmentConfiguration
            | Self::InvalidRecoveryConfiguration
            | Self::InvalidWorkerIdentity
            | Self::InvalidMailPollConfiguration
            | Self::InvalidForumSyncConfiguration
            | Self::InvalidForumReplyConfiguration
            | Self::InvalidForumConfiguration => SafeErrorKind::CONFIGURATION_INVALID,
            Self::ForumTopicProcessingFailed => SafeErrorKind::REMOTE_REJECTED,
            _ => SafeErrorKind::TASK_JOIN,
        }
    }
}
