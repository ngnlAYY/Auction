//! Worker control 传输层的模块边界。
//!
//! Worker 调度器只持有 `WorkerControlHandle` 这一窄状态端口；Unix socket、HTTP/JSON 协议、
//! peer allowlist 和 Admin client 均由本目录的 transport owner 负责，避免调度循环反向拥有
//! 进程间传输细节。

mod transport;

pub use transport::{
    LiveResult, ReadyResult, StatusResult, TriggerPollCommand, TriggerPollResult,
    WORKER_CONTROL_LIVE_PATH, WORKER_CONTROL_METRICS_PATH, WORKER_CONTROL_PROTOCOL_VERSION,
    WORKER_CONTROL_READY_PATH, WORKER_CONTROL_STATUS_PATH, WORKER_CONTROL_TRIGGER_POLL_PATH,
    WorkerControlClient, WorkerControlClientError, WorkerControlHandle, WorkerControlServer,
    WorkerControlServerConfig, WorkerControlServerError,
};
