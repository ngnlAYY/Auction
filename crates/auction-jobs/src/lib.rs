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
    AuctionScheduleRuntimeConfig, AuctionWorkerRuntimeConfig, DeliveryWorkerRuntimeConfig,
    ForumReplyPorts, ForumReplyRuntime, ForumSyncPorts, ForumSyncRuntime, ForumWorkerRuntimeConfig,
    FulfillmentWorkerRuntimeConfig, MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway,
    MailPollGatewayError, MailPollPage, MailPollRuntime, MailPollRuntimeConfig,
    OperationsAlertRuntime, OperationsAlertRuntimeConfig, PersistentRealtimeConsumer,
    PersistentWorker, RealtimeConsumerConfigError, RealtimeConsumerRunner, RecoveryHealthRuntime,
    RecoveryWorkerRuntimeConfig, WorkerError, run_persistent_until_shutdown,
    run_persistent_with_settlement_until_shutdown, run_until_shutdown,
};
