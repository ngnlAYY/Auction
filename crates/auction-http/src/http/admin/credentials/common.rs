use auction_application::{AdminCredentialError, ApplicationError};
use axum::{
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};

pub(crate) fn mutation_boundary(uri: &Uri) -> Option<Response> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "凭据命令不接受查询参数")
                .into_response(),
        );
    }
    None
}

pub(crate) fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "凭据管理只支持 PostgreSQL 完整档",
    )
    .into_response()
}

pub(crate) fn invalid_query() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response()
}

pub(crate) fn invalid_json() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效").into_response()
}

pub(crate) fn invalid_request(code: &'static str, message: &'static str) -> Response {
    super::api_error(StatusCode::BAD_REQUEST, code, message).into_response()
}

pub(crate) fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminCredential(AdminCredentialError::InvalidRequest) => {
            invalid_request("invalid_credential_request", "凭据管理参数无效")
        }
        ApplicationError::AdminCredential(AdminCredentialError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::Conflict)
        | ApplicationError::AdminCredential(AdminCredentialError::CommandConflict) => {
            super::api_error(
                StatusCode::CONFLICT,
                "credential_conflict",
                "凭据标签、指纹或命令负载与当前记录冲突",
            )
            .into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::VersionConflict { .. }) => {
            super::api_error(
                StatusCode::CONFLICT,
                "stale_credential_state",
                "凭据状态版本已变化，请重新读取后再提交",
            )
            .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::AdminCredential(AdminCredentialError::CryptoUnavailable) => {
            super::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "credential_crypto_unavailable",
                "凭据加密能力暂不可用",
            )
            .into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::StoredResultCorrupt) => {
            super::internal_error()
        }
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "凭据管理服务暂不可用",
        )
        .into_response(),
    }
}
