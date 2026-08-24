use auction_application::ApplicationError;
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
    match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "交付服务暂不可用",
        )
        .into_response(),
    }
}
