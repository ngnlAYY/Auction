//! Worker 调度实现的模块边界；运行时按职责拆分在 `runtime/`，各独立 owner
//! 保留在业务子目录，Worker control 统一由 crate 根的 `control/` 拥有。

pub(crate) mod diagnostics;
#[path = "forum/pm/mod.rs"]
mod lot_watch_pm;
mod runtime;
pub use lot_watch_pm::LotWatchPmRuntime;

pub use runtime::{
    AuctionScheduleRuntimeConfig, AuctionWorkerRuntimeConfig, DeliveryWorkerRuntimeConfig,
    ForumReplyPorts, ForumReplyRuntime, ForumSyncPorts, ForumSyncRuntime, ForumWorkerRuntimeConfig,
    FulfillmentWorkerRuntimeConfig, MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway,
    MailPollGatewayError, MailPollPage, MailPollRuntime, MailPollRuntimeBindings,
    MailPollRuntimeConfig, MailPollSchedules, OperationsAlertRuntime, OperationsAlertRuntimeConfig,
    PersistentRealtimeConsumer, PersistentWorker, PhasedMailSchedule, PlayerNotificationRuntime,
    PlayerNotificationRuntimeConfig, RealtimeConsumerConfigError, RealtimeConsumerRunner,
    RecoveryHealthRuntime, RecoveryWorkerRuntimeConfig, WorkerConnectionQuota, WorkerError,
    WorkerHotRuntime, mail_world_phase, run_persistent_with_settlement_until_shutdown,
    run_until_shutdown,
};
