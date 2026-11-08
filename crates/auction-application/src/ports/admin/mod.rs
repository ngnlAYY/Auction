//! admin 上下文。

pub(crate) mod auction_create;
pub(crate) mod auction_schedule;
pub(crate) mod auctions;
pub(crate) mod audit;
pub(crate) mod credentials;
pub(crate) mod forum;
pub(crate) mod intake;
pub(crate) mod lot_commands;
pub(crate) mod lots;
pub(crate) mod mail;
pub(crate) mod proxy_events;
pub(crate) mod renames;
pub(crate) mod rules;
pub(crate) mod runtime_config;
pub(crate) mod settlement_catalog;
pub(crate) mod users;
pub(crate) mod worlds;

pub use auction_create::{
    AdminAuctionCreatePort, AdminAuctionCreateTransaction, AdminCreateAuctionCommand,
};
pub use auction_schedule::{
    ADMIN_AUCTION_SCHEDULE_COMMAND_KIND, AdminAuctionSchedulePort, AdminAuctionScheduleSnapshot,
    AdminAuctionScheduleTransaction, AdminUpdateAuctionScheduleCommand,
};
pub use auctions::{
    ADMIN_AUCTION_STATUSES, AdminAuctionListRequest, AdminAuctionPage, AdminAuctionReader,
    AdminAuctionSummary, MAX_ADMIN_AUCTION_OFFSET, MAX_ADMIN_AUCTION_PAGE_SIZE,
};
pub use audit::{
    AdminAuditCursor, AdminAuditError, AdminAuditListRequest, AdminAuditPage, AdminAuditPort,
    AdminAuditRecord, MAX_ADMIN_AUDIT_OFFSET, MAX_ADMIN_AUDIT_PAGE_SIZE,
};
pub use credentials::{
    AdminCredential, AdminCredentialAcquirer, AdminCredentialCheckObservation,
    AdminCredentialCheckPort, AdminCredentialCheckRecord, AdminCredentialCheckResult,
    AdminCredentialCheckStatus, AdminCredentialCheckStore, AdminCredentialCommandKind,
    AdminCredentialCommandResult, AdminCredentialDeleteCommand, AdminCredentialDeleteResult,
    AdminCredentialDeleteWrite, AdminCredentialEnvelope, AdminCredentialError,
    AdminCredentialHealthReader, AdminCredentialHealthScope, AdminCredentialHealthSnapshot,
    AdminCredentialImportCommand, AdminCredentialImportSource, AdminCredentialImportWrite,
    AdminCredentialKind, AdminCredentialListRequest, AdminCredentialLogin,
    AdminCredentialMutationWrite, AdminCredentialOpener, AdminCredentialPage, AdminCredentialPort,
    AdminCredentialRenameCommand, AdminCredentialRuntimeOwnership, AdminCredentialRuntimeReader,
    AdminCredentialRuntimeRecord, AdminCredentialRuntimeWriter, AdminCredentialSealer,
    AdminCredentialSecret, AdminCredentialStatus, AdminCredentialStatusCommand,
    AdminCredentialStore, AdminCredentialVerifier, AdminCredentialWorld,
    MAX_ADMIN_CREDENTIAL_OFFSET, MAX_ADMIN_CREDENTIAL_PAGE_SIZE, MAX_ADMIN_CREDENTIAL_SECRET_BYTES,
    MailPlayerIdentityProofSource, VerifiedMailPlayerIdentity, credential_check_worlds,
    credential_scope, credential_secret_hash, valid_credential_player_name, valid_mail_player_name,
};
pub use forum::{
    AdminForumBidPost, AdminForumCatalog, AdminForumCommand, AdminForumLot, AdminForumPost,
    AdminForumPreviewError, AdminForumPreviewResult, AdminForumWarning,
};
pub use intake::{
    AdminIntakeCommand, AdminIntakeError, AdminIntakePolicy, AdminIntakePort, AdminIntakeRecord,
    AdminIntakeResult, AdminIntakeSourceKind, AdminIntakeSourceReader, AdminIntakeSourceSnapshot,
    AdminIntakeTransaction, AdminIntakeWrite, CanonicalEquipment, CanonicalEquipmentStat,
    EquipmentKeySealerPort, aggregate_id as admin_intake_aggregate_id,
};
pub use lot_commands::{
    AdminLotAction, AdminLotUpdateCommand, AdminLotUpdateError, AdminLotUpdatePort,
    AdminLotUpdateSnapshot, AdminLotUpdateTransaction,
};
pub use lots::{
    AdminLotListRequest, AdminLotPage, AdminLotReader, AdminLotSummary, MAX_ADMIN_LOT_OFFSET,
    MAX_ADMIN_LOT_PAGE_SIZE,
};
pub use mail::{
    AdminMailCommandPort, AdminMailDashboardSnapshot, AdminMailError, AdminMailIntakeDelivery,
    AdminMailIntakeDetail, AdminMailIntakeFilter, AdminMailIntakeMetadata, AdminMailIntakePage,
    AdminMailIntakeSummary, AdminMailPollState, AdminMailPollStateRequest,
    AdminMailPollTriggerCommand, AdminMailPollerSnapshot, AdminMailPollingCommand, AdminMailPort,
    AdminMailReader, AdminMailStoredCommand, AdminMailTransaction, AdminMailTransactionError,
    AdminMailTransactionFailure, AdminMailTransactionRunner, MAX_ADMIN_MAIL_DECISION_BYTES,
    MAX_ADMIN_MAIL_OFFSET, MAX_ADMIN_MAIL_PAGE_SIZE, MAX_ADMIN_MAIL_POLLER_COUNT,
    MAX_ADMIN_MAILBOX_KEY_BYTES, admin_mail_aggregate_id,
};
pub use proxy_events::{
    AdminProxyEvent, AdminProxyEventCursor, AdminProxyEventListRequest, AdminProxyEventPage,
    AdminProxyEventReader, MAX_ADMIN_PROXY_EVENT_OFFSET, MAX_ADMIN_PROXY_EVENT_PAGE_SIZE,
    UNKNOWN_ADMIN_PROXY_EVENT_TOTAL,
};
pub use renames::{
    AdminRenameListRequest, AdminRenamePage, AdminRenameReader, AdminRenameRecord,
    MAX_ADMIN_RENAME_OFFSET, MAX_ADMIN_RENAME_PAGE_SIZE,
};
pub use rules::{
    AdminRulesCandidate, AdminRulesDocument, AdminRulesError, AdminRulesPort,
    AdminRulesPublishCommand, AdminRulesReader, AdminRulesRequest, AdminRulesStoredCommand,
    AdminRulesTransaction, AdminRulesTransactionError, AdminRulesTransactionFailure,
    AdminRulesTransactionRunner, MAX_RULE_ACTOR_BYTES, MAX_RULE_MARKDOWN_BYTES,
    MAX_RULE_REQUEST_ID_BYTES, MAX_RULE_TITLE_BYTES,
};
pub use runtime_config::{
    ADMIN_RUNTIME_DIGEST_VERSION, AdminRuntimeAlertsConfig, AdminRuntimeAuditRecord,
    AdminRuntimeCommandError, AdminRuntimeCommandState, AdminRuntimeCommandStore,
    AdminRuntimeConfigError, AdminRuntimeConfigPort, AdminRuntimeDashboardMailbox,
    AdminRuntimeDeliveryConfig, AdminRuntimeDocument, AdminRuntimeEconomicsConfig,
    AdminRuntimeEquipmentFilterConfig, AdminRuntimeEquipmentLevelConfig,
    AdminRuntimeEquipmentSourceConfig, AdminRuntimeFileError, AdminRuntimeFileStore,
    AdminRuntimeForumConfig, AdminRuntimeFulfillmentConfig, AdminRuntimeListenMode,
    AdminRuntimeListenerConfig, AdminRuntimeMailConfig, AdminRuntimeMailDeliveryBudget,
    AdminRuntimeMailTemplatesConfig, AdminRuntimeMetricsConfig, AdminRuntimeOperationsConfig,
    AdminRuntimeRecoveryConfig, AdminRuntimeReplacement, AdminRuntimeScheduleConfig,
    AdminRuntimeSecurityConfig, AdminRuntimeSessionConfig, AdminRuntimeSnapshot,
    AdminRuntimeStoredCommand, AdminRuntimeTracingConfig, AdminRuntimeUpdateCommand,
    AdminRuntimeWorkerConfig, AdminRuntimeWorkerControlConfig, AdminRuntimeWorldConfig,
    AdminRuntimeWorldsConfig, MAX_ADMIN_RUNTIME_DOCUMENT_BYTES, MailTemplateError,
    WORKER_HOT_FIELD_PATHS, WORKER_RESTART_ONLY_FIELD_PATHS, WorkerDeliveryHotConfig,
    WorkerFieldEffect, WorkerForumHotConfig, WorkerHotConfig, WorkerMailHotConfig,
    valid_allowlist_host, valid_allowlist_path_prefix, valid_listener_group,
    worker_hot_field_effect,
};
pub use settlement_catalog::{
    MAX_SETTLEMENT_CATALOG_OFFSET, MAX_SETTLEMENT_CATALOG_PAGE_SIZE, SettlementCatalogEntry,
    SettlementCatalogPage, SettlementCatalogReader, SettlementCatalogRequest,
};
pub use users::{
    AdminLoginKeyIssuer, AdminLoginKeyOpener, AdminRenameRevertCommand, AdminRenameRevertWrite,
    AdminUser, AdminUserBlacklistHistoryEntry, AdminUserCommandKind, AdminUserCommandResult,
    AdminUserCreateCommand, AdminUserCreateWrite, AdminUserError, AdminUserKeyIssue,
    AdminUserKeyMode, AdminUserKeyStatus, AdminUserKeyWrite, AdminUserListItem,
    AdminUserListRequest, AdminUserMutationAction, AdminUserMutationCommand,
    AdminUserMutationResult, AdminUserMutationWrite, AdminUserPage, AdminUserPort, AdminUserStatus,
    AdminUserStore, MAX_ADMIN_USER_OFFSET, MAX_ADMIN_USER_PAGE_SIZE,
    RECOVERY_BUYER_BLACKLIST_ACTOR, RECOVERY_BUYER_BLACKLIST_LEGACY_PUBLIC_REASON,
    RECOVERY_BUYER_BLACKLIST_PROOF_KIND, RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON,
    RecoveryBuyerBlacklistCommand, RecoveryBuyerBlacklistResult, RecoveryBuyerBlacklister,
    is_recovery_buyer_blacklist_reason, recovery_buyer_blacklist_reason,
};
pub use worlds::{
    ADMIN_WORLD_SET_ENABLED_COMMAND_KIND, AdminWorldError, AdminWorldPort,
    AdminWorldSetEnabledCommand, AdminWorldTransaction, admin_world_aggregate_id,
};
