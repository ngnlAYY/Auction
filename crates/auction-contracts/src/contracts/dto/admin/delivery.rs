//! delivery 相关 DTO。

use super::*;

/// Admin delivery 安全投影；不包含 payload、lease token 或原始 result metadata。
/// `cod` 只表示经过 owner 校验后从版本化 send payload 提取的金额；其余三个结果字段同样
/// 由 owner 从版本化 metadata 派生，不把 metadata 原文传给浏览器。
/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；wire 冻结后只收紧
/// 反序列化，序列化输出（含 `skip_serializing_if` 省略规则）不变。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryResponse {
    pub id: String,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    pub cod: Option<i64>,
    pub status: String,
    pub lease_expires_at: Option<String>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub claimed_at: Option<String>,
    pub sending_at: Option<String>,
    pub completed_at: Option<String>,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    // 已发布 wire 省略 false，解码时以同一默认值补齐；null 和错误类型仍须拒绝。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub take_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returned_mail_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryListResponse {
    pub deliveries: Vec<AdminDeliveryResponse>,
    pub pagination: AdminDeliveryPaginationResponse,
}

/// Admin delivery 的旧版页码信封。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryPaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryCommandRequest {
    pub command_id: Uuid,
    pub expected_fencing_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminDeliveryCommandResponse {
    pub delivery: AdminDeliveryResponse,
}
