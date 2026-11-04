use auction_application::diagnostics::DiagnosticError;

use crate::diagnostics;

use auction_application::{ApplicationError, DeliveryAdminError};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub(crate) fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "delivery Admin 仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

pub(crate) fn map_delivery_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::Invalid) => super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_delivery_command",
            "交付管理参数无效",
        )
        .into_response(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::NotFound) => {
            super::api_error(StatusCode::NOT_FOUND, "not_found", "交付不存在").into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale) => {
            super::api_error(StatusCode::CONFLICT, "stale_delivery_state", "交付状态或版本已变化")
                .into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::SentSighted) => super::api_error(
            StatusCode::CONFLICT,
            "delivery_sent_sighted",
            "Sent 已出现该结算义务的邮件，须用人工 Sent 证明收口",
        )
        .into_response(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::CommandConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "服务暂时不可用",
        )
        .into_response(),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_delivery",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}
