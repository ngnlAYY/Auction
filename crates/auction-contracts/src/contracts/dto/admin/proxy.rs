//! proxy 相关 DTO。

use super::*;

/// Admin 代理历史的安全事件投影。
/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；wire 冻结后只收紧
/// 反序列化，序列化输出不变。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminProxyEventResponse {
    pub authorization_id: String,
    pub authorization_sequence: i64,
    pub command_id: String,
    pub created_at: String,
    pub lot_id: String,
    pub maximum_amount: i64,
    pub mode: String,
    pub state_version: i64,
    pub status: String,
    pub user_id: String,
    pub usage_number: i64,
    pub username: String,
}

/// Admin 代理历史的页码或游标分页信息。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminProxyEventPaginationResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    pub page_size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
}

/// Admin 代理历史响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminProxyEventListResponse {
    pub proxy_events: Vec<AdminProxyEventResponse>,
    pub pagination: AdminProxyEventPaginationResponse,
}
