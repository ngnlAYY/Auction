//! mail 相关 DTO。

use super::*;

/// Admin 邮件轮询状态安全响应；不包含外部 Cookie 或原始游标。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollingResponse {
    pub mailbox_key: String,
    pub last_poll_at: String,
    pub next_poll_at: String,
    pub consecutive_failures: i32,
    pub paused: bool,
    pub state_version: i64,
    pub updated_at: String,
}

/// Admin 邮箱轮询状态响应 envelope。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailStateResponse {
    pub polling: AdminMailPollingResponse,
}

/// Admin 全局邮件轮询唤醒的接受结果。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailPollTriggerResponse {
    pub command_id: String,
    pub pollers: i64,
}

/// Admin 入站邮件列表安全摘要。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailIntakeSummaryResponse {
    pub id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub kind: String,
    pub status: String,
    pub decision_code: String,
    pub received_at: String,
    pub processed_at: String,
}

/// Admin 入站邮件数字分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 入站邮件列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailIntakeListResponse {
    pub intakes: Vec<AdminMailIntakeSummaryResponse>,
    pub pagination: AdminMailPaginationResponse,
}

/// Admin 入站邮件详情的固定元数据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailIntakeMetadataResponse {
    pub auction_id: String,
    pub submission_id: String,
    pub user_id: String,
    pub added_count: i32,
    pub existing_count: i32,
    pub preview_revision: i64,
    pub attachment_count: i32,
    pub owner_result_code: String,
    pub reason_code: String,
    pub warning_codes: Vec<String>,
    pub delivery_ids: Vec<String>,
}

/// Admin 入站邮件详情响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminMailIntakeDetailResponse {
    pub id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub kind: String,
    pub status: String,
    pub decision_code: String,
    pub received_at: String,
    pub processed_at: String,
    pub metadata: AdminMailIntakeMetadataResponse,
}
