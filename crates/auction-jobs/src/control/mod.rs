//! Worker control 传输层的模块边界；调度器只持有 `WorkerControlHandle`，HTTP/JSON 协议与
//! peer allowlist 由本目录 owner 负责，socket 文件安全由装配根绑定的 listener 负责，
//! Admin client 在 `auction-integrations`。

mod mail_progress;
mod transport;

pub use mail_progress::MailPollProgressTracker;

pub use transport::{
    WorkerControlHandle, WorkerControlServer, WorkerControlServerConfig, WorkerControlServerError,
};
