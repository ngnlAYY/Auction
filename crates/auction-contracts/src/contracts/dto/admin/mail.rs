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
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub has_message_display: bool,
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
///
/// `deny_unknown_fields` 与 Admin 浏览器端详情 decoder 的闭集语义对齐；只收紧反序列化。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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

/// Admin 入站邮件关联 delivery 的安全重发响应；不包含 payload 或密钥材料。
#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailIntakeDeliveryResponse {
    pub id: String,
    pub operation: String,
    pub recipient: String,
    pub status: String,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub failure_code: Option<String>,
    pub external_reference: Option<String>,
}

/// Admin 入站邮件详情响应。
///
/// 详情 wire 是闭集（与列表摘要的兼容扩展语义不同），未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
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
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub metadata: AdminMailIntakeMetadataResponse,
    pub deliveries: Vec<AdminMailIntakeDeliveryResponse>,
    pub message_display: Option<AdminMailDisplayResponse>,
    pub equipment_diagnostic: Option<AdminMailEquipmentDiagnosticResponse>,
}

/// Admin 入站邮件原始信息的安全展示响应。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailDisplayResponse {
    pub schema_version: u8,
    pub redaction_version: u8,
    pub body: String,
    pub attachment_ids: Vec<String>,
    pub credits: i64,
    pub cod: i64,
}

impl std::fmt::Debug for AdminMailDisplayResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminMailDisplayResponse")
            .field("schema_version", &self.schema_version)
            .field("redaction_version", &self.redaction_version)
            .field("body_bytes", &self.body.len())
            .field("attachment_count", &self.attachment_ids.len())
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .finish()
    }
}

/// Admin 入站邮件装备读取诊断的独立安全响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailEquipmentDiagnosticResponse {
    pub schema_version: u8,
    pub equipment_id: String,
    pub attachment_position: u16,
    pub stage: String,
    pub category: String,
    pub detail_code: Option<String>,
    pub http_status: Option<u16>,
    pub elapsed_millis: Option<u64>,
    pub evidence_id: Option<String>,
    pub evidence_status: String,
    pub retention_error: Option<String>,
}
