//! 账号统一竞价警告渠道；不修改单件标记，也不重新路由已排队通知。
use super::*;

/// 服务器权威偏好。`channel` 是闭集 `none`/`mm`/`pm`/`both`：MM 与 PM 各自独立，可同时启用
/// （同一被超事件两个渠道各发一条），也可都关闭（标记只作收藏）；version 为正 i64。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicLotWatchPreferenceResponse {
    pub user_id: String,
    pub channel: String,
    pub version: i64,
    /// PM 渠道当前是否可用（论坛同步已启用且账号已绑定论坛账号）；不可用时启用 PM 的保存被拒绝。
    pub pm_available: bool,
}

/// 完整稳定 CAS 请求；未知结果必须原样重试。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSetLotWatchPreferenceRequest {
    pub command_id: String,
    pub channel: String,
    pub expected_version: i64,
}
