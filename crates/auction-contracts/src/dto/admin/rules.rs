//! Admin 规则文档与 runtime 配置 DTO。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::equipment::EquipmentTaxonomyDto;

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

/// Admin 规则文档安全响应；包含发布者，但不包含数据库内部模型。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRulesResponse {
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub published_by: String,
    pub revision: i64,
    pub published_at: String,
}

/// Admin 规则候选校验响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRulesCandidateResponse {
    pub locale: String,
    pub title: String,
    pub markdown: String,
}

/// Admin runtime 配置安全响应；更新成功仍明确要求重启才能让进程读取新文档。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRuntimeConfigResponse {
    pub document: Value,
    pub fingerprint: String,
    pub restart_required: bool,
    /// 仅显式请求词典投影时返回；省略时保持原 v1 响应字段集合。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equipment_taxonomy: Option<EquipmentTaxonomyDto>,
}
