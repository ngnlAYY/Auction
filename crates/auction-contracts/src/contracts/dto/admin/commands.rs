//! commands 相关 DTO。

use super::*;

/// Admin 拍卖生命周期命令请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionLifecycleRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Admin draft lot 修改请求；动作字段保持旧 HTTP surface 的字符串契约。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminLotUpdateRequest {
    pub command_id: Uuid,
    pub expected_state_version: i64,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_price: Option<i64>,
}

/// Admin 邮箱暂停/恢复轮询请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollingRequest {
    pub command_id: Uuid,
    pub mailbox_key: String,
    pub paused: bool,
    pub expected_state_version: i64,
}

/// Admin 立即触发一次邮箱轮询的请求。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollTriggerRequest {
    pub command_id: Uuid,
}
