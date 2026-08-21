use serde::Serialize;

/// 健康检查响应。
///
/// `service` 与 `version` 都由调用方传入：`env!("CARGO_PKG_VERSION")` 在本 crate 内展开只会
/// 得到 `auction-contracts` 自己的版本，而运维需要的是运行进程的版本。当前工作区版本统一，
/// 两者取值恰好相同，但语义不同——独立发布版本后差异就会显现。
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub service: String,
    pub status: &'static str,
    pub version: &'static str,
}

impl HealthResponse {
    /// 构造健康响应。`version` 应由二进制装配根传入自身的 `CARGO_PKG_VERSION`。
    pub fn ok(service: impl Into<String>, version: &'static str) -> Self {
        Self { service: service.into(), status: "ok", version }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiErrorResponse {
    pub code: &'static str,
    pub message: String,
    pub request_id: Option<String>,
}
