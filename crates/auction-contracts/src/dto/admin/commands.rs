//! commands 相关 DTO。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::response::deserialize_null_default;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionLifecycleRequest {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollingRequest {
    pub command_id: Uuid,
    pub mailbox_key: String,
    pub paused: bool,
    pub expected_state_version: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminMailPollTriggerRequest {
    pub command_id: Uuid,
}
