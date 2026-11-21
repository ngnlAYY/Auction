//! Admin 寄售响应外壳及安全错误映射。

use auction_application::consignment::{ConsignmentError, ListingBusinessRejection};
use auction_contracts::ApiResponse;
use axum::Json;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::errors::api_error;
use crate::middleware::boundary::request_id;

/// 在送出成功状态前由 DTO 校验完整投影；异常端口结果仅返回固定错误。
pub(super) fn ok(
    message: &'static str,
    data: impl Serialize,
) -> Result<Response, ConsignmentError> {
    let Ok(bytes) = serde_json::to_vec(&ApiResponse::ok(message, data)) else {
        return Err(ConsignmentError::Unavailable);
    };
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

/// 按 Admin 路由的错误闭集应答。超出闭集的端口错误没有对应的契约，按依赖异常失败关闭。
pub(super) fn error(headers: &HeaderMap, failure: ConsignmentError) -> Response {
    let (failure, message) = match failure {
        ConsignmentError::Business(ListingBusinessRejection::ListingNotFound) => {
            (failure, "挂单不存在")
        }
        ConsignmentError::Business(ListingBusinessRejection::StaleVersion) => {
            (failure, "挂单版本已变化，请刷新后确认")
        }
        ConsignmentError::Business(ListingBusinessRejection::ListingNotActive) => {
            (failure, "挂单已停止接受操作")
        }
        ConsignmentError::CommandIdConflict => (failure, "命令编号已用于不同请求"),
        ConsignmentError::InvalidCommandId => (failure, "命令编号无效"),
        ConsignmentError::InvalidListingId => (failure, "挂单编号无效"),
        ConsignmentError::InvalidListingRequest => (failure, "挂单请求无效"),
        ConsignmentError::InvalidReason => (failure, "下架说明无效"),
        ConsignmentError::InvalidQuery => (failure, "挂单查询参数无效"),
        ConsignmentError::OutcomeUnknown => (failure, "无法确认挂单命令结果，请用原命令原样重试"),
        _ => (ConsignmentError::Unavailable, "一口价服务暂不可用"),
    };
    let (status, code) = failure.api_error().unwrap_or((503, "consignment_unavailable"));
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    json_error(headers, status, code, message)
}

/// 沿用 HTTP 边界已规范化的关联号；错误消息只使用调用方提供的固定文案。
pub(super) fn json_error(
    headers: &HeaderMap,
    status: StatusCode,
    code: &'static str,
    message: &'static str,
) -> Response {
    let (status, Json(mut body)) = api_error(status, code, message);
    body.request_id = request_id(headers);
    (status, Json(body)).into_response()
}
