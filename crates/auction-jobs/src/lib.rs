//! Worker 任务编排的薄 crate 入口。

mod control;
mod worker;

pub use control::{
    LiveResult, MailPollProgressTracker, ReadyResult, StatusResult, TriggerPollCommand,
    TriggerPollResult, WORKER_CONTROL_LIVE_PATH, WORKER_CONTROL_MAIL_PROGRESS_PATH,
    WORKER_CONTROL_METRICS_PATH, WORKER_CONTROL_PROTOCOL_VERSION, WORKER_CONTROL_READY_PATH,
    WORKER_CONTROL_STATUS_PATH, WORKER_CONTROL_TRIGGER_POLL_PATH, WorkerControlClient,
    WorkerControlClientError, WorkerControlHandle, WorkerControlServer, WorkerControlServerConfig,
    WorkerControlServerError,
};

pub use worker::LotWatchPmRuntime;
pub use worker::{
    ASSET_RECONCILIATION_INTERVAL, ASSET_RECONCILIATION_TIMEOUT, AssetReconciliationRuntime,
    AuctionScheduleRuntimeConfig, AuctionWorkerRuntimeConfig, DeliveryWorkerRuntimeConfig,
    ForumReplyPorts, ForumReplyRuntime, ForumSyncPorts, ForumSyncRuntime, ForumWorkerRuntimeConfig,
    FulfillmentWorkerRuntimeConfig, MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway,
    MailPollGatewayError, MailPollPage, MailPollRuntime, MailPollRuntimeBindings,
    MailPollRuntimeConfig, MailPollSchedules, OperationsAlertRuntime, OperationsAlertRuntimeConfig,
    OperationsAlertWebhookRuntime, PersistentWorker, PhasedMailSchedule, PlayerNotificationRuntime,
    PlayerNotificationRuntimeConfig, RecoveryHealthRuntime, RecoveryWorkerRuntimeConfig,
    WorkerConnectionQuota, WorkerError, WorkerHotRuntime, mail_world_phase,
    run_persistent_with_settlement_until_shutdown, run_until_shutdown,
};

mod runtime_switches;
pub use runtime_switches::start_runtime_switch_refresh;

mod sli;
pub use sli::start_sli_snapshot;
