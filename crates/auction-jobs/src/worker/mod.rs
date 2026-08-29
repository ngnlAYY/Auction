//! Worker 调度实现的模块边界。
//!
//! `runtime/` 按配置、错误、主调度、delivery、recovery、生命周期和结算职责拆分 Worker
//! 运行时；各类独立 owner 继续放在 `auction/`、`forum/`、`mail/`、`realtime/`、`recovery/`、
//! `settlement/` 和 `operations/` 子目录。Worker control 不在这里声明，统一由 crate 根的
//! `control/` 传输边界拥有。

mod runtime;

pub use runtime::{
    AuctionScheduleRuntimeConfig, AuctionWorkerRuntimeConfig, DeliveryWorkerRuntimeConfig,
    ForumReplyRuntime, ForumSyncRuntime, ForumWorkerRuntimeConfig, FulfillmentWorkerRuntimeConfig,
    MailPollFetchRequest, MailPollFetchedMessage, MailPollGateway, MailPollGatewayError,
    MailPollPage, MailPollRuntime, MailPollRuntimeConfig, OperationsAlertRuntime,
    OperationsAlertRuntimeConfig, PersistentRealtimeConsumer, PersistentWorker,
    RealtimeConsumerConfigError, RealtimeConsumerRunner, RecoveryHealthRuntime,
    RecoveryWorkerRuntimeConfig, WorkerError, run_persistent_until_shutdown,
    run_persistent_with_settlement_until_shutdown, run_until_shutdown,
};
