//! Worker 任务编排的薄 crate 入口。

mod control;
mod worker;

pub use control::{
    MailPollProgressTracker, WorkerControlHandle, WorkerControlServer, WorkerControlServerConfig,
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
    WorkerConnectionQuota, WorkerError, WorkerHotBaseline, WorkerHotGroup, WorkerHotRuntime,
    mail_world_phase, run_persistent_with_settlement_until_shutdown, run_until_shutdown,
};

mod mail_templates;
pub use mail_templates::start_mail_template_refresh;

mod runtime_switches;
pub use runtime_switches::start_runtime_switch_refresh;

mod sli;
pub use sli::start_sli_snapshot;
