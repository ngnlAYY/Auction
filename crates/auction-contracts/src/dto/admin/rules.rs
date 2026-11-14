//! rules 相关 DTO。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Admin 规则候选校验请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRulesCandidateRequest {
    pub locale: String,
    pub title: String,
    pub markdown: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRulesPublishRequest {
    pub command_id: Uuid,
    pub expected_revision: i64,
    pub locale: String,
    pub title: String,
    pub markdown: String,
}

/// Admin runtime 配置候选校验请求；文档值由应用层反序列化为严格的非敏感 TOML 模型。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRuntimeConfigValidateRequest {
    pub document: Value,
}

/// Admin runtime 配置更新请求；fingerprint 是当前文件快照的版本 fence。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRuntimeConfigUpdateRequest {
    pub command_id: Uuid,
    pub expected_fingerprint: String,
    pub document: Value,
}

/// Admin 改名历史安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRenameResponse {
    pub id: String,
    pub user_id: String,
    pub user_state_version: i64,
    pub previous_username: String,
    pub next_username: String,
    pub actor: String,
    pub created_at: String,
    pub reverted_at: Option<String>,
}

/// Admin 改名历史数字分页。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRenamePaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin 改名历史列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRenameListResponse {
    pub renames: Vec<AdminRenameResponse>,
    pub pagination: AdminRenamePaginationResponse,
}
