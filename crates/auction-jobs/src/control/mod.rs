//! Worker control 传输层的模块边界；调度器只持有 `WorkerControlHandle`，Unix socket、
//! HTTP/JSON 协议与 peer allowlist 由本目录 owner 负责，Admin client 在 `auction-integrations`。

mod mail_progress;
mod transport;

pub use mail_progress::MailPollProgressTracker;

pub use transport::{
    LiveResult, ReadyResult, StatusResult, TriggerPollCommand, TriggerPollResult,
    WORKER_CONTROL_EXTERNAL_ACCOUNTS_PATH, WORKER_CONTROL_LIVE_PATH,
    WORKER_CONTROL_MAIL_PROGRESS_PATH, WORKER_CONTROL_METRICS_PATH,
    WORKER_CONTROL_PROTOCOL_VERSION, WORKER_CONTROL_READY_PATH, WORKER_CONTROL_STATUS_PATH,
    WORKER_CONTROL_TRIGGER_POLL_PATH, WorkerControlHandle, WorkerControlServer,
    WorkerControlServerConfig, WorkerControlServerError,
};
