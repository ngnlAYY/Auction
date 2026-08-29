//! realtime 相关 DTO。

use super::*;

/// Admin realtime lag 的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeLagResponse {
    pub stream_key: String,
    pub consumer_key: String,
    pub offset: i64,
    pub latest_sequence: i64,
    pub lag: i64,
}

/// Admin realtime lag 列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeLagListResponse {
    pub lags: Vec<AdminRealtimeLagResponse>,
    pub pagination: AdminRealtimePaginationResponse,
}

/// Admin realtime lag 的旧版页码信封。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimePaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

/// Admin realtime failure 的安全投影；不包含原始驱动错误或未过滤 payload。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeFailureResponse {
    pub id: String,
    pub event_id: String,
    pub stream_key: String,
    pub consumer_key: String,
    pub code: String,
    pub safe_detail: String,
    pub failed_at: String,
    pub resolved_at: Option<String>,
    pub resolved: bool,
}

/// Admin realtime failure 列表响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeFailureListResponse {
    pub failures: Vec<AdminRealtimeFailureResponse>,
    pub pagination: AdminRealtimeFailurePaginationResponse,
}

/// Admin realtime failure 的旧版页码信封。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeFailurePaginationResponse {
    pub page: i64,
    pub page_size: i64,
    pub has_more: bool,
    pub total: i64,
}

/// Admin realtime failure 详情响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeFailureDetailResponse {
    pub failure: AdminRealtimeFailureResponse,
}

/// Admin realtime failure replay 请求。
///
/// 新客户端应在重试中复用 `command_id`；旧版请求没有该字段，HTTP 防腐层会按 failure 与
/// 修正负载稳定派生。payload 只允许应用事件注册表接受的公开对象。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdminRealtimeReplayRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<Uuid>,
    pub schema_version: i32,
    pub public_payload: Value,
}

/// Admin realtime replay 追加事件的安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeReplayEventResponse {
    pub event_id: String,
    pub replay_of_event_id: String,
    pub stream_key: String,
    pub stream_sequence: i64,
    pub kind: String,
    pub schema_version: i32,
    pub created_at: String,
}

/// Admin realtime replay 响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct AdminRealtimeReplayResponse {
    pub replay: AdminRealtimeReplayEventResponse,
}
