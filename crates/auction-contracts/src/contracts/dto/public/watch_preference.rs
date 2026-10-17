//! 账号统一竞价警告渠道；不修改单件标记，也不重新路由已排队通知。
use super::*;

/// 服务器权威偏好，channel 为 mm/pm，version 为正 i64。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicLotWatchPreferenceResponse {
    pub user_id: String,
    pub channel: String,
    pub version: i64,
}

/// 完整稳定 CAS 请求；未知结果必须原样重试。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSetLotWatchPreferenceRequest {
    pub command_id: String,
    pub channel: String,
    pub expected_version: i64,
}
