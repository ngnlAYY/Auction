//! Admin 邮件诊断的可选扩展；仅包含经过筛选的数字 MID 和运行观测。

use serde::{Deserialize, Serialize};

/// 通过显式查询选择的诊断，不含原始 cursor、claim token、发件人、正文或凭据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollingDiagnosticsResponse {
    pub observed_at: String,
    /// available / empty / hidden；hidden 不回传原始值。
    pub cursor_status: String,
    pub completed_external_mail_id: Option<String>,
    /// available / unavailable / timeout / incompatible / rejected。
    pub worker_condition: String,
    pub worker_observed_at: Option<String>,
    pub progress: Option<AdminMailPollProgressResponse>,
}

/// 进程内入站阶段；刷新时读取，不代表外部发送或附件动作完成。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollProgressResponse {
    pub mailbox_key: String,
    pub world_id: String,
    pub stage: String,
    pub external_mail_id: Option<String>,
    pub updated_at: String,
    pub last_failure: Option<AdminMailPollProgressFailureResponse>,
}

/// 最近一次失败定位；Worker 重启后清空，不能据此自动跳过或重发。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollProgressFailureResponse {
    pub stage: String,
    pub external_mail_id: Option<String>,
    pub occurred_at: String,
}
