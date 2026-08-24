//! Worker 调度实现的模块边界。
//!
//! `runtime.rs` 拥有调度循环、任务配置和 Worker 运行时状态；各类独立 owner 继续放在
//! `auction/`、`forum/`、`mail/`、`realtime/`、`recovery/`、`settlement/` 和 `operations/`
//! 子目录。Worker control 不在这里声明，统一由 crate 根的 `control/` 传输边界拥有。

mod runtime;

pub use runtime::{
    AuctionWorkerRuntimeConfig, DeliveryWorkerRuntimeConfig, ForumReplyRuntime, ForumSyncRuntime,
    ForumWorkerRuntimeConfig, FulfillmentWorkerRuntimeConfig, MailPollFetchRequest,
    MailPollFetchedMessage, MailPollGateway, MailPollGatewayError, MailPollPage, MailPollRuntime,
    MailPollRuntimeConfig, OperationsAlertRuntime, OperationsAlertRuntimeConfig,
    PersistentRealtimeConsumer, PersistentWorker, RealtimeConsumerConfigError,
    RealtimeConsumerRunner, RecoveryHealthRuntime, RecoveryWorkerRuntimeConfig, WorkerError,
    run_persistent_until_shutdown, run_persistent_with_settlement_until_shutdown,
    run_until_shutdown,
};
