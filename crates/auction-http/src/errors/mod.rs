//! HTTP 稳定错误外壳：`ApiErrorResponse` 信封与查询拒绝、能力缺失、依赖不可用、资源不存在
//! 这几类跨 handler 共用的状态码与 code。

use auction_contracts::ApiErrorResponse;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::diagnostics;

/// 构造稳定错误信封，并按状态码与 code 记录响应诊断；`message` 只取固定文案，不回显输入。
pub(crate) fn api_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> (StatusCode, Json<ApiErrorResponse>) {
    diagnostics::response_error(status, Some(code));
    (
        status,
        Json(ApiErrorResponse {
            code: code.to_owned(),
            message: message.to_owned(),
            data: None,
            request_id: None,
        }),
    )
}

/// 稳定 400：查询参数或筛选值不合法，使用通用提示。需要更具体的提示时改用 [`invalid_query`]。
pub(crate) fn query_rejection() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
}

/// 稳定 400：查询参数或筛选值不合法，`message` 为该路由的固定提示。
pub(crate) fn invalid_query(message: &'static str) -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_query", message).into_response()
}

/// [`feature_unavailable`] 的通用原因：未配置数据库时进程以运行期骨架启动，依赖数据库的端口
/// 都没有装配。只依赖额外密钥或文件的端口由各自边界写明缺少的配置项。
pub(crate) const DATABASE_NOT_CONFIGURED: &str = "未配置数据库，该能力不可用";
/// [`feature_unavailable`] 的通用原因：端口已装配，但返回 `CapabilityUnavailable`，表示当前
/// 部署不提供该操作；与端口缺失区分，避免把运维引向已经配置好的数据库或密钥。
pub(crate) const CAPABILITY_NOT_PROVIDED: &str = "当前部署未提供该能力";

/// 稳定 501：当前进程不提供该能力。`message` 按原因取固定提示（缺少的配置项，或
/// [`CAPABILITY_NOT_PROVIDED`]），只供运维排障；客户端只依据 code 分支。
pub(crate) fn feature_unavailable(message: &'static str) -> Response {
    api_error(StatusCode::NOT_IMPLEMENTED, "feature_not_supported", message).into_response()
}

/// 稳定 503：数据库或依赖暂时不可用。
pub(crate) fn storage_unavailable(message: &'static str) -> Response {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", message).into_response()
}

/// 稳定 404：请求的资源不存在或对当前调用方不可见。
pub(crate) fn not_found_error() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::NOT_FOUND, "not_found", "请求的资源不存在")
}
