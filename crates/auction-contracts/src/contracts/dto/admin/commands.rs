//! commands 相关 DTO。

use super::*;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminAuctionLifecycleRequest {
    #[serde(default, deserialize_with = "null_as_default")]
    pub command_id: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub expected_state_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
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
