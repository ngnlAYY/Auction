//! Worker control 的 Unix socket 协议常量：协议版本、请求标识头与全部只读/命令路径。
//! 服务端（`auction-jobs`）与客户端（`auction-integrations` 及 Admin 装配）共用同一份
//! 定义，路径或版本变化必须同时评估旧客户端的兼容处理。

/// 当前 Worker control HTTP/JSON 协议版本。
pub const WORKER_CONTROL_PROTOCOL_VERSION: &str = "v1";
/// 请求与响应携带的协议版本头。
pub const WORKER_CONTROL_HEADER_VERSION: &str = "x-worker-control-version";
/// 请求关联标识头；服务端回显同一值。
pub const WORKER_CONTROL_HEADER_REQUEST_ID: &str = "x-request-id";

pub const WORKER_CONTROL_LIVE_PATH: &str = "/v1/health/live";
pub const WORKER_CONTROL_READY_PATH: &str = "/v1/health/ready";
pub const WORKER_CONTROL_STATUS_PATH: &str = "/v1/status";
/// 只读的进程内邮件观测，不包含原始游标或 claim 凭据。
pub const WORKER_CONTROL_MAIL_PROGRESS_PATH: &str = "/v1/mail/progress";
/// 只读的外部账号运行时快照；不包含凭据、Cookie、URL 或页面正文。
pub const WORKER_CONTROL_EXTERNAL_ACCOUNTS_PATH: &str = "/v1/external-accounts";
/// Worker control MoogleMail 唤醒路径。
pub const WORKER_CONTROL_TRIGGER_POLL_PATH: &str = "/v1/mail/polls:trigger";
/// Admin 手工装备 URL 读取经 Worker 执行。
pub const WORKER_CONTROL_EQUIPMENT_READ_PATH: &str = "/v1/equipment/reads";
/// CLI `mail inspect-sent` 的远端 Sent 只读核查经 Worker 执行。
pub const WORKER_CONTROL_SENT_LOOKUP_PATH: &str = "/v1/mail/sent:lookup";
/// 恢复后外部核对在全局暂停下扫描 Sent、Read Mail 与论坛新楼层。
pub const WORKER_CONTROL_RECOVERY_SCAN_PATH: &str = "/v1/recovery/external:scan";

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::mail::MailPollProgressSnapshot;

/// 存活探针的线上结果。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerControlLiveResult {
    pub state: String,
}

/// 就绪探针的线上结果。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerControlReadyResult {
    pub ready: bool,
}

/// Worker 状态响应数据；不包含配置、凭据或错误正文。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerControlStatusResult {
    pub mail_pollers: usize,
    pub ready: bool,
    pub state: String,
    pub version: String,
}

/// 独立只读路径的进度响应，避免改变旧 status 的闭集。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerControlMailProgressResult {
    pub observed_at: DateTime<Utc>,
    pub pollers: Vec<MailPollProgressSnapshot>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerControlTriggerPollCommand {
    pub command_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerControlTriggerPollResult {
    pub accepted: bool,
    pub command_id: String,
    pub pollers: usize,
}

/// Worker control 的响应外壳；服务端与客户端共用同一形状。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerControlEnvelope<T> {
    pub code: String,
    pub message: String,
    pub data: Option<T>,
}
