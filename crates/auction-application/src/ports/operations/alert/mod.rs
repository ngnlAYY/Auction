//! operations alert 的领域安全投影、阈值规则和窄存储端口。
//! 告警不是拍卖事实，也不能改变竞价、结算或履约结果。该模块只把已经由各 owner 提供的
//! 健康摘要转换成稳定 signal，并通过独立端口交给 PostgreSQL 保存。通知入队仍然
//! 发生在扫描事务之后，避免外部 MoogleMail 结果反向决定告警事实。

/// 单轮最多允许冻结的告警 signal 数量。
pub const MAX_OPERATIONS_ALERT_SIGNALS: usize = 64;
/// 单个告警批次最多包含的 signal 数量。
pub const MAX_OPERATIONS_ALERT_BATCH_ITEMS: usize = 64;
/// 告警批次正文的最大字节数。
pub const MAX_OPERATIONS_ALERT_BODY_BYTES: usize = 16 << 10;

mod assessment;
mod console;
mod delivery;
mod digest;
mod disposition_read;
mod evaluate;
mod health;
mod host;
mod model;
mod signals;
mod webhook;

pub use assessment::{
    DeliveryAssessment, FulfillmentAssessment, RecoveryAssessment, assess_delivery,
    assess_fulfillment, assess_recovery,
};
pub use console::{
    AdminPendingCounts, MAX_OPERATIONS_ALERT_CONSOLE_OFFSET,
    MAX_OPERATIONS_ALERT_CONSOLE_PAGE_SIZE, MAX_OPERATIONS_ALERT_SILENCE_SECONDS,
    MAX_OPERATIONS_ALERT_WEBHOOK_VIEW, MIN_OPERATIONS_ALERT_SILENCE_SECONDS,
    OperationsAlertAcknowledgeCommand, OperationsAlertCommandResult, OperationsAlertConsole,
    OperationsAlertConsoleEntry, OperationsAlertConsolePage, OperationsAlertConsoleQuery,
    OperationsAlertFilter, OperationsAlertSilenceCommand, OperationsAlertWebhookEntry,
    parse_alert_id,
};
pub use delivery::{
    OperationsAlertBatchClaim, OperationsAlertBatchItem, OperationsAlertClaimRequest,
    OperationsAlertDeliveryStatus, OperationsAlertDispatchErrorCode, OperationsAlertEnqueuedBatch,
    OperationsAlertHostObserveCommand, OperationsAlertObserveCommand,
};
pub use digest::{body_digest, canonical_summary, canonical_summary_prefix, request_hash};
pub use disposition_read::{
    DispositionReadCounts, DispositionReadField, DispositionReadMode, DispositionReadSelection,
    SHADOW_OWNER, select_read_counts,
};
pub use evaluate::evaluate_snapshot;
pub use health::{
    OperationsAlertHealth, OperationsAlertRuntimeStatus, OperationsAlertSourceReader,
    OperationsAlertStore,
};
pub use host::{
    OperationsAlertHostBackupFacts, OperationsAlertHostBackupOutcome, OperationsAlertHostBackupRun,
    OperationsAlertHostDisk, OperationsAlertHostRestoreDrill, OperationsAlertHostWalFacts,
    evaluate_host_backup, evaluate_host_wal,
};
pub use model::{
    OperationsAlertCredentialSnapshot, OperationsAlertDeliverySnapshot,
    OperationsAlertForumSnapshot, OperationsAlertFulfillmentSnapshot,
    OperationsAlertLifecycleSnapshot, OperationsAlertMailSnapshot,
    OperationsAlertNotificationSnapshot, OperationsAlertPolicy, OperationsAlertProcessSnapshot,
    OperationsAlertProcessState, OperationsAlertReconciliationSnapshot,
    OperationsAlertReconciliationWorld, OperationsAlertRecoverySnapshot,
    OperationsAlertRecoveryWorld, OperationsAlertSnapshot, OperationsAlertSourceObservation,
    OperationsAlertTarget,
};
pub use signals::{
    OPERATIONS_ALERT_ACCOUNT_BUSY_AGE, OPERATIONS_ALERT_DIGEST_INTERVAL,
    OPERATIONS_ALERT_LIFECYCLE_GRACE, OPERATIONS_ALERT_MAIL_POLL_STALL_AGE,
    OPERATIONS_ALERT_QUEUE_LATENCY_AGE, OPERATIONS_ALERT_RECONCILIATION_STALE_AGE,
    OPERATIONS_ALERT_RESTORE_DRILL_MAX_AGE, OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE,
    OperationsAlertError, OperationsAlertKind, OperationsAlertSeverity, OperationsAlertSignal,
    OperationsAlertSource,
};
pub use webhook::{
    DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE, MAX_OPERATIONS_ALERT_WEBHOOK_ATTEMPTS,
    OperationsAlertWebhookClaimRequest, OperationsAlertWebhookDispatch,
    OperationsAlertWebhookFinish, OperationsAlertWebhookOutcome, OperationsAlertWebhookSender,
    render_webhook_payload, validate_webhook_template,
};
