//! Worker control 传输层的模块边界；调度器只持有 `WorkerControlHandle`，Unix socket、
//! HTTP/JSON 协议、peer allowlist 与 Admin client 由本目录 owner 负责。

mod transport;

pub use transport::{
    LiveResult, ReadyResult, StatusResult, TriggerPollCommand, TriggerPollResult,
    WORKER_CONTROL_LIVE_PATH, WORKER_CONTROL_METRICS_PATH, WORKER_CONTROL_PROTOCOL_VERSION,
    WORKER_CONTROL_READY_PATH, WORKER_CONTROL_STATUS_PATH, WORKER_CONTROL_TRIGGER_POLL_PATH,
    WorkerControlClient, WorkerControlClientError, WorkerControlHandle, WorkerControlServer,
    WorkerControlServerConfig, WorkerControlServerError,
};
