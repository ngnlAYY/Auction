//! commands 相关 DTO。

use super::*;

/// Public 普通竞价请求。命令 ID 必须由调用方生成并在重试中保持不变。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceBidRequest {
    pub command_id: Uuid,
    pub amount: i64,
}

/// Public 代理授权请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProxyAuthorizationRequest {
    pub command_id: Uuid,
    pub maximum_amount: i64,
    pub mode: String,
}

/// Public 降低代理上限请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LowerProxyAuthorizationRequest {
    pub command_id: Uuid,
    pub maximum_amount: i64,
}

/// 状态命令的最小确认数据，供前端区分接受、重放和冲突。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct CommandAcknowledgement {
    pub command_id: Uuid,
    pub result_code: String,
}
