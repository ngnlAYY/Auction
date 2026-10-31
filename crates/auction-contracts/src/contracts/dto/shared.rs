//! 通用响应、健康探针、错误 DTO 与字段反序列化边界。

use super::*;

/// 字段必须显式出现，但可为 `null`；调用方不得附加 `default` 放宽缺失字段。
pub(super) fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// 复现 Go 非指针标量的 `null` 零值语义；缺失字段由调用方的 `default` 单独控制。
/// 类型错误仍拒绝，零值交给业务边界生成原有错误分类。
pub(super) fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// 旧系统统一响应外壳的成功形式。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct ApiResponse<T> {
    pub code: String,
    pub message: String,
    pub data: T,
}

impl<T> ApiResponse<T> {
    /// 构造稳定的成功响应；业务数据仍由 surface 自己定义。
    pub fn ok(message: impl Into<String>, data: T) -> Self {
        Self { code: "ok".to_owned(), message: message.into(), data }
    }

    /// 构造旧 v1 创建成功响应；HTTP 201 与业务 `code=created` 必须同时保持。
    pub fn created(message: impl Into<String>, data: T) -> Self {
        Self { code: "created".to_owned(), message: message.into(), data }
    }

    /// 构造表示异步命令已接受的响应；它不表示外部副作用已经完成。
    pub fn accepted(message: impl Into<String>, data: T) -> Self {
        Self { code: "accepted".to_owned(), message: message.into(), data }
    }
}

/// `service` 与 `version` 由装配根传入，避免探针误用本契约 crate 的进程身份与版本。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct HealthResponse {
    pub service: String,
    pub status: String,
    pub version: String,
}

impl HealthResponse {
    /// 构造健康响应。`version` 应由二进制装配根传入自身的 `CARGO_PKG_VERSION`。
    pub fn ok(service: impl Into<String>, version: &'static str) -> Self {
        Self { service: service.into(), status: "ok".to_owned(), version: version.to_owned() }
    }
}

/// 与旧版 `/health/live`、`/health/ready` 保持一致的探针响应外壳。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct HealthProbeResponse<T> {
    pub code: String,
    pub message: String,
    pub data: T,
}

/// 进程存活探针数据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct HealthLiveData {
    pub live: bool,
}

/// 进程就绪探针数据。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct HealthReadyData {
    pub ready: bool,
}

/// Public 持久化实时事件的安全信封。
/// 负载已经由应用层事件注册表校验；这里不暴露数据库内部状态版本列或 origin 字段。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicRealtimeEventResponse {
    pub event_id: String,
    pub stream_sequence: i64,
    pub kind: String,
    pub schema_version: i32,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicRealtimeResponse {
    pub auction_id: String,
    pub events: Vec<PublicRealtimeEventResponse>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

/// 旧版每拍卖回放合同只接受 `auction_id`；新版需要的发现读取以显式 `scope=auctions`
/// 扩展提供，避免把省略 `auction_id` 偷换成另一个 v1 语义。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct PublicRealtimeDiscoveryResponse {
    pub scope: String,
    pub events: Vec<PublicRealtimeEventResponse>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct ApiErrorResponse {
    pub code: String,
    pub message: String,
    /// 可选的安全业务数据；`None` 仍固定序列化为旧契约要求的 `data: null`。
    #[serde(default)]
    pub data: Option<Value>,
    pub request_id: Option<String>,
}

/// 带稳定业务数据的错误响应，用于封禁原因等旧契约投影。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub struct ApiErrorResponseWithData<T> {
    pub code: String,
    pub message: String,
    pub data: T,
    pub request_id: Option<String>,
}
