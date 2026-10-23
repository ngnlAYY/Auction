//! account 相关 DTO。

use super::*;

/// Public 普通竞价成功后的稳定结果。
/// 结果来自竞价事务提交后的 lot 投影；重放同一 `command_id` 时返回同一份投影，不把数据库
/// 计划或内部竞价事实直接暴露给浏览器。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PlaceBidResponse {
    pub bid: PlaceBidResultResponse,
}

/// Public 普通竞价结果中的公开 lot 状态。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PlaceBidResultResponse {
    pub lot_id: String,
    pub current_price: i64,
    pub effective_end_at: String,
    pub state_version: i64,
    pub leading: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedBidResponse {
    pub id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub amount: i64,
    pub accepted_state_version: i64,
    pub fact_index: i64,
    pub mechanism: String,
    pub created_at: String,
    pub leading: bool,
    pub equipment: PublicOwnedEquipmentResponse,
}

/// 账户投影中的最小装备信息。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedEquipmentResponse {
    pub name: String,
    pub category: String,
}

/// 账户代理授权 quota。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedProxyQuotaResponse {
    pub used: i64,
    pub limit: i64,
}

/// 账户代理授权的历史转换事实。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedProxyTransitionResponse {
    pub transition_number: i64,
    pub kind: String,
    pub old_maximum_amount: Option<i64>,
    pub new_maximum_amount: i64,
    pub old_mode: Option<String>,
    pub new_mode: String,
    pub old_status: Option<String>,
    pub new_status: String,
    pub reason: Option<String>,
    pub resulting_state_version: i64,
    pub created_at: String,
}

/// 账户代理授权当前可执行能力。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedProxyCapabilitiesResponse {
    pub can_lower: bool,
    pub waiting: bool,
    pub at_maximum: bool,
}

/// 当前用户自己的代理授权投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicOwnedProxyResponse {
    pub authorization_id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub mode: String,
    pub maximum_amount: i64,
    pub quota: PublicOwnedProxyQuotaResponse,
    pub transitions: Vec<PublicOwnedProxyTransitionResponse>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub equipment: PublicOwnedEquipmentResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid: Option<PublicOwnedBidResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
    pub capabilities: PublicOwnedProxyCapabilitiesResponse,
}

/// 该 DTO 对齐旧系统命令响应的最小公开字段；它不携带账户页专属的拍卖、装备、时间和
/// capability 字段，也不把命令账本的原始结果暴露给浏览器。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicProxyCommandBidResponse {
    pub lot_id: String,
    pub current_price: i64,
    pub effective_end_at: String,
    pub state_version: i64,
    pub leading: bool,
}

/// Public 代理命令成功后的授权投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicProxyCommandAuthorizationResponse {
    pub authorization_id: String,
    pub lot_id: String,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub mode: String,
    pub maximum_amount: i64,
    pub quota: PublicOwnedProxyQuotaResponse,
    pub transitions: Vec<PublicOwnedProxyTransitionResponse>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    pub state_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid: Option<PublicProxyCommandBidResponse>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicProxyCommandResponse {
    pub authorization: PublicProxyCommandAuthorizationResponse,
}

/// 当前用户竞价与代理历史列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicMyBidsResponse {
    pub bids: Vec<PublicOwnedBidResponse>,
    pub proxies: Vec<PublicOwnedProxyResponse>,
    pub bid_total: i64,
    pub proxy_total: i64,
    pub page: i64,
    pub bid_page: i64,
    pub proxy_page: i64,
    pub page_size: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicMyProxyResponse {
    pub authorization: Option<PublicOwnedProxyResponse>,
}

/// Public 登录请求；原始 key 只在 HTTP 到应用端口的短路径中存在，调试输出必须脱敏。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicLoginRequest {
    pub key: String,
}

impl std::fmt::Debug for PublicLoginRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("PublicLoginRequest").field("key", &"[REDACTED]").finish()
    }
}

/// Public 登录结果；两个截止字段同时为 null 表示可撤销的永久会话。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicLoginResponse {
    pub user_id: String,
    pub username: String,
    pub status: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub expires_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub absolute_expires_at: Option<String>,
}

/// Public 权威会话；永久会话仍受账号、凭据和撤销状态约束。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicSessionCurrentResponse {
    pub authenticated: bool,
    pub user_id: String,
    pub username: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub expires_at: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub absolute_expires_at: Option<String>,
}

/// 账号后续登录时长及当前会话实际截止；偏好与现有其他设备会话相互独立。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSessionPreferenceResponse {
    pub user_id: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub duration_seconds: Option<i64>,
    pub version: i64,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub expires_at: Option<String>,
}

/// 更新当前会话与后续登录时长；必须显式提交 null 才选择永久，缺字段不能扩大有效期。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSetSessionPreferenceRequest {
    pub command_id: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub duration_seconds: Option<i64>,
    pub expected_version: i64,
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；服务端两条路径都只
/// 序列化本 DTO，收紧只影响反序列化；调试输出隐藏 CSRF token，不影响授权响应字段。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSessionResponse {
    pub authenticated: bool,
    pub actor: String,
    pub csrf_token: String,
    pub expires_at: String,
    pub absolute_expires_at: String,
}

impl std::fmt::Debug for AdminSessionResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSessionResponse")
            .field("authenticated", &self.authenticated)
            .field("actor", &self.actor)
            .field("csrf_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("absolute_expires_at", &self.absolute_expires_at)
            .finish()
    }
}

/// Public 黑名单响应中的可公开原因。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicAccountBlockedResponse {
    pub public_reason: String,
}

/// Public 世界条目的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct WorldStatusResponse {
    pub enabled: bool,
}

/// Public 世界目录响应，保持旧系统以世界 ID 为 key 的 JSON 结构。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct WorldsResponse {
    pub worlds: BTreeMap<String, WorldStatusResponse>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicRulesResponse {
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub revision: i64,
    pub published_at: String,
}

/// Admin 规则文档安全响应；包含发布者，但不包含数据库内部模型。
/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；只收紧反序列化。
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
/// `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；只收紧反序列化。
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

/// Public 黑名单条目的稳定响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicBlacklistEntryResponse {
    pub username: String,
    pub reason: String,
    pub blacklisted_at: String,
}

/// Public 黑名单分页响应，保持旧系统的 `users` 字段命名。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicBlacklistResponse {
    pub users: Vec<PublicBlacklistEntryResponse>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}
