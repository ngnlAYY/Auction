//! Worker 任务编排的薄 crate 入口。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod control;
mod worker;

pub use control::{
    MailPollProgressTracker, WorkerControlHandle, WorkerControlServer, WorkerControlServerConfig,
    WorkerControlServerError,
};

pub use worker::LotWatchPmRuntime;
pub use worker::consignment;
pub use worker::{
    ASSET_RECONCILIATION_INTERVAL, ASSET_RECONCILIATION_TIMEOUT, AssetReconciliationRuntime,
    AuctionScheduleRuntimeConfig, AuctionWorkerRuntimeConfig, DeliveryWorkerRuntimeConfig,
    ForumReplyPorts, ForumReplyRuntime, ForumSyncPorts, ForumSyncRuntime, ForumWorkerRuntimeConfig,
    FulfillmentWorkerRuntimeConfig, MailPollConsignmentCandidate, MailPollFetchRequest,
    MailPollFetchedMessage, MailPollGateway, MailPollGatewayError, MailPollPage, MailPollRuntime,
    MailPollRuntimeBindings, MailPollRuntimeConfig, MailPollSchedules, OperationsAlertRuntime,
    OperationsAlertRuntimeConfig, OperationsAlertWebhookRuntime, PersistentWorker,
    PlayerNotificationRuntime, PlayerNotificationRuntimeConfig, RecoveryHealthRuntime,
    RecoveryWorkerRuntimeConfig, WorkerConnectionQuota, WorkerError, WorkerHotBaseline,
    WorkerHotGroup, WorkerHotRuntime, run_persistent_with_settlement_until_shutdown,
    run_until_shutdown,
};

mod mail_templates;
pub use mail_templates::start_mail_template_refresh;

mod runtime_switches;
pub use runtime_switches::start_runtime_switch_refresh;

mod sli;
pub use sli::start_sli_snapshot;
