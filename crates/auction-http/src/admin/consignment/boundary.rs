//! Admin 寄售响应外壳及安全错误映射。

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, ListingBusinessRejection,
};
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

/// 返回 202 已接受结果；回执编码失败时写结果仍未知。
pub(super) fn accepted(
    message: &'static str,
    data: impl Serialize,
) -> Result<Response, ConsignmentError> {
    let Ok(bytes) = serde_json::to_vec(&ApiResponse::accepted(message, data)) else {
        return Err(ConsignmentError::OutcomeUnknown);
    };
    Ok((StatusCode::ACCEPTED, [(header::CONTENT_TYPE, "application/json")], bytes).into_response())
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
        ConsignmentError::InvalidDepositRequest => (failure, "寄售收件请求无效"),
        ConsignmentError::InvalidReason => (failure, "下架说明无效"),
        ConsignmentError::InvalidQuery => (failure, "挂单查询参数无效"),
        ConsignmentError::OutcomeUnknown => (failure, "无法确认挂单命令结果，请用原命令原样重试"),
        ConsignmentError::Deposit(rejection) => (failure, deposit_message(rejection)),
        // v1 Admin 既有错误闭集没有禁用码；v2 新业务使用独立的 404 `disabled`。
        ConsignmentError::Disabled => (ConsignmentError::Unavailable, "一口价服务暂不可用"),
        _ => (ConsignmentError::Unavailable, "一口价服务暂不可用"),
    };
    let (status, code) = failure.api_error().unwrap_or((503, "consignment_unavailable"));
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
    json_error(headers, status, code, message)
}

/// v2 收件/隔离/停售路由的完整错误闭集；v1 保持原有受限映射。
pub(super) fn deposit_error(headers: &HeaderMap, failure: ConsignmentError) -> Response {
    if let ConsignmentError::Business(rejection) = failure {
        let (status, code) = failure.api_error().unwrap_or((503, "consignment_unavailable"));
        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
        return json_error(headers, status, code, listing_message(rejection));
    }
    error(headers, failure)
}

fn listing_message(rejection: ListingBusinessRejection) -> &'static str {
    match rejection {
        ListingBusinessRejection::ListingNotFound => "挂单不存在",
        ListingBusinessRejection::ParticipantIneligible => "参与者不具备寄售资格",
        ListingBusinessRejection::AlreadyOpen => "该装备已有挂单",
        ListingBusinessRejection::EquipmentBusy => "装备仍被其他事实占用",
        ListingBusinessRejection::HistoryUnproven => "寄售历史不足",
        ListingBusinessRejection::HistoryOwnerMismatch => "寄售历史所有权不匹配",
        ListingBusinessRejection::HistoryAliasConflict => "寄售历史别名冲突",
        ListingBusinessRejection::FeeChanged => "寄售费率已变化",
        ListingBusinessRejection::StaleVersion => "挂单版本已变化，请刷新后确认",
        ListingBusinessRejection::ListingNotActive => "挂单已停止接受操作",
        ListingBusinessRejection::WorldUnavailable => "寄售世界暂不可用",
        ListingBusinessRejection::RegistrationRateLimited => "寄售登记次数已达上限",
        ListingBusinessRejection::ListingLimitReached => "寄售挂单数量已达上限",
    }
}

/// v2 新业务在首次禁用时返回 404；v1 既有错误码保持原映射。
pub(super) fn disabled(headers: &HeaderMap) -> Response {
    json_error(headers, StatusCode::NOT_FOUND, "consignment_disabled", "一口价服务已关闭")
}

fn deposit_message(rejection: DepositBusinessRejection) -> &'static str {
    match rejection {
        DepositBusinessRejection::StaleVersion => "隔离版本已变化，请刷新后确认",
        DepositBusinessRejection::ListingNotActive => "挂单已停止接受该操作",
        DepositBusinessRejection::ActionNotAllowed => "当前状态不允许该寄售操作",
        DepositBusinessRejection::MailUnavailable => "寄售收件能力暂不可用",
        DepositBusinessRejection::EvidenceUnproven => "隔离证据不足，仍保持隔离",
        DepositBusinessRejection::EvidenceConflict => "隔离证据互相矛盾，仍保持隔离",
        DepositBusinessRejection::DispositionConflict => "已存在相反的寄售最终处置",
        DepositBusinessRejection::HoldMismatch => "运营止售与当前有效止售不一致",
        DepositBusinessRejection::SourceMismatch => "隔离来源与本操作不匹配",
        DepositBusinessRejection::EquipmentBusy => "装备仍被其他事实占用",
        DepositBusinessRejection::ParticipantIneligible => "参与者不具备寄售资格",
        DepositBusinessRejection::BotParty => "认证 Bot 不能作为寄售交易当事人",
        DepositBusinessRejection::SeasonClosed => "寄售季已关闭",
        DepositBusinessRejection::FenceMismatch => "资产起写围栏已失效",
    }
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
