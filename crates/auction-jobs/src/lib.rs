//! Worker 任务编排的薄 crate 入口。

mod worker;

pub use worker::{
    PersistentRealtimeConsumer, PersistentWorker, RealtimeConsumerConfigError,
    RealtimeConsumerRunner, WorkerError, run_persistent_until_shutdown,
    run_persistent_with_settlement_until_shutdown, run_until_shutdown,
};
