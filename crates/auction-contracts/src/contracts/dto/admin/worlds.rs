//! Admin 业务世界目录与启停 DTO。

use super::*;

/// Admin 可见的业务世界权威状态。
///
/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；只收紧反序列化。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminWorldResponse {
    pub id: String,
    pub enabled: bool,
    pub availability: String,
    pub policy_revision: i64,
}

/// Admin 业务世界目录响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminWorldListResponse {
    pub worlds: Vec<AdminWorldResponse>,
}

/// Admin 业务世界启停请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminWorldSetEnabledRequest {
    pub command_id: Uuid,
    pub enabled: bool,
    pub expected_policy_revision: i64,
}

/// Admin 业务世界启停成功响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminWorldSetEnabledResponse {
    pub world: AdminWorldResponse,
}
