//! 寄售响应外壳与只含安全分类的 HTTP 错误映射。

use auction_application::consignment::ConsignmentError;
use auction_contracts::{ApiResponse, CommandReceipt, ListingDisposition};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::errors::api_error;

pub(super) type RouteResult = Result<Response, RouteError>;

pub(super) enum RouteError {
    Consignment(ConsignmentError),
    Guard(Box<Response>),
    BodyTooLarge,
    UnsupportedMediaType,
}

impl From<ConsignmentError> for RouteError {
    fn from(error: ConsignmentError) -> Self {
        Self::Consignment(error)
    }
}

impl From<Box<Response>> for RouteError {
    fn from(response: Box<Response>) -> Self {
        Self::Guard(response)
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Guard(response) => return *response,
            Self::BodyTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", "请求体过大"),
            Self::UnsupportedMediaType => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type", "请求必须使用 JSON")
            }
            Self::Consignment(error) => {
                // Worker 专属分类不属于 Public；错误端口投影按依赖失败关闭。
                let (status, code) = error.api_error().unwrap_or((503, "consignment_unavailable"));
                let status =
                    StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
                let message = match error {
                    ConsignmentError::Disabled => "一口价服务已关闭",
                    ConsignmentError::OutcomeUnknown => "无法确认挂单命令结果",
                    _ if status.is_server_error() => "一口价服务暂不可用",
                    _ => "挂单请求未完成",
                };
                (status, code, message)
            }
        };
        api_error(status, code, message).into_response()
    }
}

/// 返回以 DTO 严格序列化的读结果。
pub(super) fn loaded(data: impl Serialize) -> RouteResult {
    success(StatusCode::OK, ApiResponse::ok("已加载", data), ConsignmentError::Unavailable)
}

/// 初次接受和重放共用同一回执、状态与成功文案。
pub(super) fn command(receipt: Result<CommandReceipt, ConsignmentError>) -> RouteResult {
    let failure = ConsignmentError::OutcomeUnknown;
    let Ok(receipt) = receipt else { return Err(failure.into()) };
    let message = match receipt.disposition {
        ListingDisposition::Registered => "登记已受理",
        ListingDisposition::Repriced => "已改价",
        ListingDisposition::Unchanged => "标价未变更",
        ListingDisposition::Closed => "挂单已取消",
        ListingDisposition::Rejected => return Err(failure.into()),
    };
    if receipt.disposition == ListingDisposition::Registered {
        success(StatusCode::ACCEPTED, ApiResponse::accepted(message, receipt), failure)
    } else {
        success(StatusCode::OK, ApiResponse::ok(message, receipt), failure)
    }
}

fn success<T: Serialize>(
    status: StatusCode,
    response: ApiResponse<T>,
    failure: ConsignmentError,
) -> RouteResult {
    // DTO 在序列化时验证自洽性。先编码，在读投影失效时保留稳定 503；
    // 写回执无法编码时保留结果未知，客户端仍可查原回执或原样重试。
    let Ok(bytes) = serde_json::to_vec(&response) else { return Err(failure.into()) };
    Ok((status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}
