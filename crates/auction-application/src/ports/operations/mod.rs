//! 跨用例的进程生命周期窄端口。

mod external_write;
pub use external_write::*;

pub(crate) mod alert;
pub(crate) mod readiness;
pub(crate) mod worker_control;
pub(crate) mod worker_remote_read;
pub use alert::{
    AdminPendingCounts, MAX_OPERATIONS_ALERT_BATCH_ITEMS, MAX_OPERATIONS_ALERT_BODY_BYTES,
    MAX_OPERATIONS_ALERT_CONSOLE_OFFSET, MAX_OPERATIONS_ALERT_CONSOLE_PAGE_SIZE,
    MAX_OPERATIONS_ALERT_SIGNALS, MAX_OPERATIONS_ALERT_SILENCE_SECONDS,
    MIN_OPERATIONS_ALERT_SILENCE_SECONDS, OPERATIONS_ALERT_ACCOUNT_BUSY_AGE,
    OPERATIONS_ALERT_DIGEST_INTERVAL, OPERATIONS_ALERT_LIFECYCLE_GRACE,
    OPERATIONS_ALERT_MAIL_POLL_STALL_AGE, OPERATIONS_ALERT_QUEUE_LATENCY_AGE,
    OPERATIONS_ALERT_RECONCILIATION_STALE_AGE, OPERATIONS_ALERT_WAL_UPLOAD_STALL_AGE,
    OperationsAlertAcknowledgeCommand, OperationsAlertBatchClaim, OperationsAlertBatchItem,
    OperationsAlertClaimRequest, OperationsAlertCommandResult, OperationsAlertConsole,
    OperationsAlertConsoleEntry, OperationsAlertConsolePage, OperationsAlertConsoleQuery,
    OperationsAlertCredentialSnapshot, OperationsAlertDeliverySnapshot,
    OperationsAlertDeliveryStatus, OperationsAlertDispatchErrorCode, OperationsAlertEnqueuedBatch,
    OperationsAlertError, OperationsAlertFilter, OperationsAlertForumSnapshot,
    OperationsAlertFulfillmentSnapshot, OperationsAlertHealth, OperationsAlertHostBackupFacts,
    OperationsAlertHostBackupOutcome, OperationsAlertHostBackupRun, OperationsAlertHostDisk,
    OperationsAlertHostObserveCommand, OperationsAlertHostWalFacts, OperationsAlertKind,
    OperationsAlertLifecycleSnapshot, OperationsAlertMailSnapshot,
    OperationsAlertNotificationSnapshot, OperationsAlertObserveCommand, OperationsAlertPolicy,
    OperationsAlertProcessSnapshot, OperationsAlertProcessState,
    OperationsAlertReconciliationSnapshot, OperationsAlertReconciliationWorld,
    OperationsAlertRecoverySnapshot, OperationsAlertRecoveryWorld, OperationsAlertRuntimeStatus,
    OperationsAlertSeverity, OperationsAlertSignal, OperationsAlertSilenceCommand,
    OperationsAlertSnapshot, OperationsAlertSource, OperationsAlertSourceObservation,
    OperationsAlertSourceReader, OperationsAlertStore, OperationsAlertTarget, body_digest,
    canonical_summary, canonical_summary_prefix, evaluate_host_backup, evaluate_host_wal,
    evaluate_snapshot, parse_alert_id, request_hash,
};
pub use readiness::{AuditFailureMode, MigrationCompatibilityMode, ProcessReadinessPort};
pub use worker_control::{
    WorkerControlCondition, WorkerControlMailProgress, WorkerControlPort, WorkerControlStatus,
    WorkerControlTrigger,
};
pub use worker_remote_read::{
    MAX_RECOVERY_FORUM_THREADS, MAX_RECOVERY_SCAN_ENTRIES, MAX_RECOVERY_SCAN_PAGES,
    MAX_RECOVERY_SENT_DETAILS, UnavailableWorkerRemoteRead, WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS,
    WORKER_REMOTE_READ_TIMEOUT_SECONDS, WorkerEquipmentReadRequest, WorkerEquipmentReadResponse,
    WorkerRecoveryForumPost, WorkerRecoveryForumTarget, WorkerRecoveryInboundEntry,
    WorkerRecoveryMailRef, WorkerRecoveryScanRequest, WorkerRecoveryScanResponse,
    WorkerRecoverySentEntry, WorkerRemoteReadError, WorkerRemoteReadFuture, WorkerRemoteReadPort,
    WorkerSentLookupCandidate, WorkerSentLookupRequest, WorkerSentLookupResponse,
};
