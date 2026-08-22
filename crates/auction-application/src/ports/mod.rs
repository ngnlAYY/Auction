use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{
    Auction, AuctionId, AuctionRules, CompetitionPlan, LotBidState, LotId, ProxyAuthorization,
    UserId,
};

mod admin_auction_create;
mod admin_auctions;
mod admin_audit;
mod admin_forum;
mod admin_intake;
mod admin_lot_commands;
mod admin_lots;
mod admin_mail;
mod admin_proxy_events;
mod admin_renames;
mod admin_rules;
mod admin_runtime_config;
mod blacklist;
mod commands;
mod compensation;
mod delivery;
mod delivery_admin;
mod forum;
mod fulfillment;
mod lifecycle;
mod mail_poll;
mod queries;
mod realtime;
mod realtime_admin;
mod realtime_consumer;
mod recovery;
mod recovery_report;
mod rules;
mod scheduler;
mod sessions;
mod settlement;
mod settlement_report;
mod settlement_scheduler;

pub use admin_auction_create::{
    AdminAuctionCreatePort, AdminAuctionCreateTransaction, AdminCreateAuctionCommand,
};
pub use admin_auctions::{
    ADMIN_AUCTION_STATUSES, AdminAuctionListRequest, AdminAuctionPage, AdminAuctionReader,
    AdminAuctionSummary, MAX_ADMIN_AUCTION_OFFSET, MAX_ADMIN_AUCTION_PAGE_SIZE,
};
pub use admin_audit::{
    AdminAuditCursor, AdminAuditError, AdminAuditListRequest, AdminAuditPage, AdminAuditPort,
    AdminAuditRecord, MAX_ADMIN_AUDIT_OFFSET, MAX_ADMIN_AUDIT_PAGE_SIZE,
};
pub use admin_forum::{
    AdminForumBidPost, AdminForumCatalog, AdminForumCommand, AdminForumLot, AdminForumPost,
    AdminForumPreviewCommand, AdminForumPreviewError, AdminForumPreviewPort,
    AdminForumPreviewResult, AdminForumWarning, MAX_ADMIN_FORUM_PREVIEW_BYTES,
};
pub use admin_intake::{
    AdminIntakeCommand, AdminIntakeError, AdminIntakePort, AdminIntakeRecord, AdminIntakeResult,
    AdminIntakeSourceKind, AdminIntakeSourceReader, AdminIntakeSourceSnapshot,
    AdminIntakeTransaction, AdminIntakeWrite, CanonicalEquipment, CanonicalEquipmentStat,
    EquipmentKeySealerPort, aggregate_id as admin_intake_aggregate_id,
};
pub use admin_lot_commands::{
    AdminLotAction, AdminLotUpdateCommand, AdminLotUpdateError, AdminLotUpdatePort,
    AdminLotUpdateSnapshot, AdminLotUpdateTransaction,
};
pub use admin_lots::{
    AdminLotListRequest, AdminLotPage, AdminLotReader, AdminLotSummary, MAX_ADMIN_LOT_OFFSET,
    MAX_ADMIN_LOT_PAGE_SIZE,
};
pub use admin_mail::{
    AdminMailCommandPort, AdminMailError, AdminMailIntakeDetail, AdminMailIntakeFilter,
    AdminMailIntakeMetadata, AdminMailIntakePage, AdminMailIntakeSummary, AdminMailPollState,
    AdminMailPollStateRequest, AdminMailPollTriggerCommand, AdminMailPollingCommand, AdminMailPort,
    AdminMailReader, AdminMailStoredCommand, AdminMailTransaction, AdminMailTransactionError,
    AdminMailTransactionFailure, AdminMailTransactionRunner, MAX_ADMIN_MAIL_DECISION_BYTES,
    MAX_ADMIN_MAIL_OFFSET, MAX_ADMIN_MAIL_PAGE_SIZE, MAX_ADMIN_MAILBOX_KEY_BYTES,
    admin_mail_aggregate_id,
};
pub use admin_proxy_events::{
    AdminProxyEvent, AdminProxyEventCursor, AdminProxyEventListRequest, AdminProxyEventPage,
    AdminProxyEventReader, MAX_ADMIN_PROXY_EVENT_OFFSET, MAX_ADMIN_PROXY_EVENT_PAGE_SIZE,
    UNKNOWN_ADMIN_PROXY_EVENT_TOTAL,
};
pub use admin_renames::{
    AdminRenameListRequest, AdminRenamePage, AdminRenameReader, AdminRenameRecord,
    MAX_ADMIN_RENAME_OFFSET, MAX_ADMIN_RENAME_PAGE_SIZE,
};
pub use admin_rules::{
    AdminRulesCandidate, AdminRulesDocument, AdminRulesError, AdminRulesPort,
    AdminRulesPublishCommand, AdminRulesReader, AdminRulesRequest, AdminRulesStoredCommand,
    AdminRulesTransaction, AdminRulesTransactionError, AdminRulesTransactionFailure,
    AdminRulesTransactionRunner, MAX_RULE_ACTOR_BYTES, MAX_RULE_MARKDOWN_BYTES,
    MAX_RULE_REQUEST_ID_BYTES, MAX_RULE_TITLE_BYTES,
};
pub use admin_runtime_config::{
    ADMIN_RUNTIME_DIGEST_VERSION, AdminRuntimeAuditRecord, AdminRuntimeCommandError,
    AdminRuntimeCommandState, AdminRuntimeCommandStore, AdminRuntimeConfigError,
    AdminRuntimeConfigPort, AdminRuntimeDocument, AdminRuntimeEquipmentSourceConfig,
    AdminRuntimeFileError, AdminRuntimeFileStore, AdminRuntimeListenMode,
    AdminRuntimeListenerConfig, AdminRuntimeMetricsConfig, AdminRuntimeReplacement,
    AdminRuntimeSecurityConfig, AdminRuntimeSessionConfig, AdminRuntimeSnapshot,
    AdminRuntimeStoredCommand, AdminRuntimeTracingConfig, AdminRuntimeUpdateCommand,
    AdminRuntimeWorkerControlConfig, MAX_ADMIN_RUNTIME_DOCUMENT_BYTES,
};
pub use blacklist::{
    MAX_PUBLIC_BLACKLIST_OFFSET, MAX_PUBLIC_BLACKLIST_PAGE_SIZE, PublicBlacklistEntry,
    PublicBlacklistPage, PublicBlacklistReader, PublicBlacklistRequest,
};
pub use commands::{ApplicationFuture, CompetitionCommandPort};
pub use compensation::{
    DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE, DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE,
    MAX_PUBLIC_COMPENSATION_OFFSET, MAX_PUBLIC_COMPENSATION_PAGE_SIZE, PublicCompensationCursor,
    PublicCompensationListRequest, PublicCompensationMovement, PublicCompensationMovementKind,
    PublicCompensationMovementPage, PublicCompensationMovementRequest, PublicCompensationPage,
    PublicCompensationReader, PublicCompensationRound,
};
pub use delivery::{
    DeliverySendResult, DeliveryTransport, DeliveryTransportError, DeliveryWorkClaim,
    DeliveryWorkStore,
};
pub use delivery_admin::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminListRequest, DeliveryAdminPage,
    DeliveryAdminPort, DeliveryAdminResult, DeliveryAdminSnapshot, MAX_DELIVERY_ADMIN_PAGE_SIZE,
};
pub use forum::{PUBLIC_FORUM_STATUSES, PublicForumReader, PublicForumThread};
pub use fulfillment::{
    SettlementDeliveryPreparation, SettlementFulfillmentActivationResult,
    SettlementFulfillmentClaim, SettlementFulfillmentCommandPort, SettlementFulfillmentOperation,
    SettlementFulfillmentResult, SettlementFulfillmentState, SettlementFulfillmentStore,
};
pub use lifecycle::{
    AuctionLifecycleAction, AuctionLifecycleActor, AuctionLifecycleActorType,
    AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecycleResult,
    AuctionLifecycleTransaction,
};
pub use mail_poll::{
    MAX_MAIL_POLL_BATCH_SIZE, MAX_MAIL_POLL_DECISION_BYTES, MAX_MAIL_POLL_FAILURE_CODE_BYTES,
    MAX_MAIL_POLL_FAILURE_DETAIL_BYTES, MailPollBatch, MailPollClaim, MailPollIntake,
    MailPollStore,
};
pub use queries::{
    AuctionReader, CurrentAuctionSnapshot, DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE,
    MAX_PUBLIC_ACCOUNT_OFFSET, MAX_PUBLIC_ACCOUNT_PAGE_SIZE, MAX_PUBLIC_BID_LOG_OFFSET,
    MAX_PUBLIC_BID_LOG_PAGE_SIZE, MAX_PUBLIC_HISTORY_NAME_TERMS, MAX_PUBLIC_LOT_OFFSET,
    MAX_PUBLIC_LOT_PAGE_SIZE, MAX_PUBLIC_PRICE_HISTORY_OFFSET, MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE,
    PUBLIC_LOT_CATEGORIES, PublicAccountPageRequest, PublicAccountReader, PublicAuctionCursor,
    PublicAuctionListRequest, PublicAuctionPage, PublicAuctionProjection, PublicAuctionSummary,
    PublicBidLogCursor, PublicBidLogEntry, PublicBidLogListRequest, PublicBidLogPage,
    PublicBidLogReader, PublicEquipment, PublicEquipmentStat, PublicFinalRestriction,
    PublicLiveLotDetail, PublicLiveLotSummary, PublicLotDetail, PublicLotListRequest,
    PublicLotPage, PublicLotReader, PublicMyBid, PublicMyBidsPage, PublicMyBidsRequest,
    PublicOwnedProxy, PublicPreviewLotDetail, PublicPreviewLotSummary, PublicPriceHistoryCursor,
    PublicPriceHistoryEntry, PublicPriceHistoryListRequest, PublicPriceHistoryPage,
    PublicPriceHistoryReader, PublicPriceHistoryStats, PublicProxyBid, PublicProxyCapabilities,
    PublicProxyTransition, WorldReader, WorldSnapshot,
};
pub use realtime::{
    MAX_PUBLIC_EVENT_PAGE_SIZE, PublicRealtimeEvent, PublicRealtimePage, PublicRealtimeReader,
    PublicRealtimeRequest,
};
pub use realtime_admin::{
    MAX_REALTIME_ADMIN_PAGE_SIZE, RealtimeAdminError, RealtimeAdminPort,
    RealtimeFailureListRequest, RealtimeFailurePage, RealtimeFailureSnapshot,
    RealtimeLagListRequest, RealtimeLagPage, RealtimeLagSnapshot, RealtimeReplayCommand,
    RealtimeReplayResult,
};
pub use realtime_consumer::{
    MAX_REALTIME_CONSUMER_BATCH_SIZE, RealtimeConsumerBatch, RealtimeConsumerRequest,
    RealtimeConsumerStore, RealtimeEventSink, RealtimeSinkError,
};
pub use recovery::{
    RecoveryCommandError, RecoveryCommandPort, RecoveryCommandRepository, RecoveryCommandService,
    RecoveryConfirmationAction, RecoveryConfirmationCommand, RecoveryConfirmationResult,
};
pub use recovery_report::{
    RecoveryActionReport, RecoveryReport, RecoveryReportEntry, RecoveryReportReader,
    allowed_recovery_commands,
};
pub use rules::{
    PUBLIC_RULES_DEFAULT_LOCALE, PUBLIC_RULES_LOCALES, PublicRulesDocument, PublicRulesReader,
    PublicRulesRequest,
};
pub use scheduler::{ScheduledRoundClaim, ScheduledRoundPhase, ScheduledRoundStore};
pub use sessions::{
    AdminLoginResult, AdminSessionConfig, AdminSessionIdentity, AdminSessionPort,
    AdminSessionRecord, AdminSessionStore, PublicLoginAuthenticator, PublicLoginIdentity,
    PublicLoginResult, PublicSessionConfig, PublicSessionIdentity, PublicSessionPort,
    PublicSessionRecord, PublicSessionStore,
};
pub use settlement::{
    SettlementSnapshot, SettlementTransaction, SettlementTransactionError,
    SettlementTransactionFailure, SettlementTransactionRunner,
};
pub use settlement_report::{
    SettlementAdjustment, SettlementAuctionSnapshot, SettlementLotReport, SettlementReport,
    SettlementReportReader, SettlementSummary,
};
pub use settlement_scheduler::{SettlementWorkClaim, SettlementWorkStore};

/// PostgreSQL 与队列消费路径允许使用的事务隔离级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionIsolation {
    Serializable,
    ReadCommitted,
}

/// 应用层传给存储适配器的事务策略。
///
/// 竞价和关键生命周期写入使用 Serializable，最多尝试三次；队列领取等已经由显式行锁
/// 保护的路径才使用 Read Committed。这个类型不允许适配器把两种语义悄悄合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionPolicy {
    pub isolation: TransactionIsolation,
    pub max_attempts: u8,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TransactionPolicyError {
    #[error("事务至少需要一次尝试")]
    NoAttempts,
    #[error("Serializable 事务最多允许三次尝试")]
    SerializableAttemptsExceeded,
    #[error("Read Committed 路径不允许隐式重试")]
    ReadCommittedRetry,
}

impl TransactionPolicy {
    /// 竞价写入和关键生命周期写入的固定策略。
    pub const fn serializable() -> Self {
        Self { isolation: TransactionIsolation::Serializable, max_attempts: 3 }
    }

    /// 队列领取等通过显式锁保护的短事务策略。
    pub const fn read_committed() -> Self {
        Self { isolation: TransactionIsolation::ReadCommitted, max_attempts: 1 }
    }

    /// 从配置或适配器输入构造并校验事务策略。
    pub const fn new(
        isolation: TransactionIsolation,
        max_attempts: u8,
    ) -> Result<Self, TransactionPolicyError> {
        if max_attempts == 0 {
            return Err(TransactionPolicyError::NoAttempts);
        }
        match isolation {
            TransactionIsolation::Serializable if max_attempts > 3 => {
                Err(TransactionPolicyError::SerializableAttemptsExceeded)
            }
            TransactionIsolation::ReadCommitted if max_attempts != 1 => {
                Err(TransactionPolicyError::ReadCommittedRetry)
            }
            _ => Ok(Self { isolation, max_attempts }),
        }
    }
}

/// 幂等账本使用的固定 SHA-256 请求摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestHash([u8; 32]);

impl RequestHash {
    /// 从已经计算好的 SHA-256 摘要建立值对象。
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// 返回摘要字节，适配器负责写入固定 32 字节字段。
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }

    /// 按稳定的命令种类、聚合标识和请求 DTO 计算 SHA-256 摘要。
    ///
    /// 摘要算法属于应用幂等边界而不是 HTTP 层；这样重试、队列和其他入口不会各自定义
    /// 一套容易漂移的 hash 规则。
    pub fn from_request<T: Serialize>(
        command_kind: &str,
        aggregate_id: Uuid,
        request: &T,
    ) -> Result<Self, serde_json::Error> {
        #[derive(Serialize)]
        struct Envelope<'a, T> {
            command_kind: &'a str,
            aggregate_id: Uuid,
            request: &'a T,
        }

        let payload = serde_json::to_vec(&Envelope { command_kind, aggregate_id, request })?;
        let digest = Sha256::digest(payload);
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&digest);
        Ok(Self::new(bytes))
    }
}

/// 命令账本中用于重放的最小记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCommand {
    pub command_id: auction_domain::CommandId,
    pub command_kind: String,
    pub aggregate_id: Uuid,
    pub request_hash: RequestHash,
    pub result_code: String,
    pub result_payload: Vec<u8>,
}

impl StoredCommand {
    /// 校验账本记录对应的非空字段和固定摘要长度。
    pub fn validate(&self) -> Result<(), CommandLedgerError> {
        if self.command_id.is_nil()
            || self.aggregate_id.is_nil()
            || self.command_kind.trim().is_empty()
            || self.result_code.trim().is_empty()
            || self.result_payload.is_empty()
        {
            return Err(CommandLedgerError::InvalidRecord);
        }
        if serde_json::from_slice::<Value>(&self.result_payload)
            .ok()
            .is_none_or(|payload| !payload.is_object())
        {
            return Err(CommandLedgerError::InvalidRecord);
        }
        Ok(())
    }

    /// 判断传入命令是否与已保存请求完全相同。
    pub fn matches_request(
        &self,
        command_kind: &str,
        aggregate_id: Uuid,
        request_hash: RequestHash,
    ) -> bool {
        self.command_kind == command_kind
            && self.aggregate_id == aggregate_id
            && self.request_hash == request_hash
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CommandLedgerError {
    #[error("命令账本记录无效")]
    InvalidRecord,
    #[error("命令 ID 与已有请求负载冲突")]
    Conflict,
    #[error("命令账本存储失败")]
    Storage,
}

/// 命令账本端口。实现必须在持有业务写锁的同一事务中完成查重与写入。
pub trait CommandLedger: Send + Sync {
    fn find(
        &self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CommandLedgerError>> + Send;

    fn insert(
        &self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CommandLedgerError>> + Send;
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository operation failed: {0}")]
    Failed(String),
}

pub trait AuctionRepository: Send + Sync {
    fn insert(&self, auction: Auction) -> impl Future<Output = Result<(), RepositoryError>> + Send;

    fn get(
        &self,
        id: AuctionId,
    ) -> impl Future<Output = Result<Option<Auction>, RepositoryError>> + Send;
}

/// 竞价命令在锁内读取到的最小聚合快照。
#[derive(Debug, Clone)]
pub struct CompetitionSnapshot {
    pub rules: AuctionRules,
    pub state: LotBidState,
    pub active_proxy: Option<ProxyAuthorization>,
}

#[derive(Debug, Error)]
pub enum CompetitionTransactionError {
    #[error("竞价投影不存在")]
    NotFound,
    #[error("竞价事务中的资源状态发生冲突")]
    Conflict,
    #[error("管理 lot 的状态版本已经过期")]
    StaleState,
    #[error("管理 lot 当前状态不允许该命令")]
    InvalidState,
    #[error("竞价投影读取或写入失败: {0}")]
    Failed(String),
    #[error("竞价事务发生可重试的序列化冲突")]
    SerializationConflict,
    #[error(transparent)]
    CommandLedger(#[from] CommandLedgerError),
}

/// 允许事务 runner 判断一次应用失败是否应按事务策略重试。
pub trait TransactionFailure:
    From<CompetitionTransactionError> + From<TransactionPolicyError> + Send
{
    fn is_serialization_conflict(&self) -> bool;
}

/// 事务回调的异步返回类型，使 runner 能在提交前继续持有事务对象。
pub type TransactionFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// 竞价事务对象。实现必须在同一数据库事务中锁定快照并原子应用计划。
pub trait CompetitionTransaction: Send {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send;

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

    fn load_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<CompetitionSnapshot, CompetitionTransactionError>> + Send;

    /// 在已锁定 lot 的事务中计算该用户下一次代理授权的使用次数。
    fn next_proxy_usage_number(
        &mut self,
        lot_id: LotId,
        user_id: UserId,
    ) -> impl Future<Output = Result<u64, CompetitionTransactionError>> + Send;

    fn apply_plan(
        &mut self,
        plan: CompetitionPlan,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// 事务 runner 端口。实现负责开始、提交/回滚和后端隔离级别映射。
pub trait TransactionRunner: Send + Sync {
    type Transaction: CompetitionTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: TransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send;
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
