use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

use auction_application::admin::AdminCredentialError;
use auction_application::core::ApplicationError;
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
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminCredential(AdminCredentialError::InvalidRequest) => {
            invalid_request("invalid_credential_request", "凭据管理参数无效")
        }
        ApplicationError::AdminCredential(AdminCredentialError::InvalidCookie) => {
            invalid_request("invalid_credential_secret", "Cookie 认证字段无效")
        }
        ApplicationError::AdminCredential(AdminCredentialError::LoginRejected) => super::api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "credential_login_rejected",
            "论坛拒绝账号密码登录",
        )
        .into_response(),
        ApplicationError::AdminCredential(AdminCredentialError::LoginChallenge) => {
            super::api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "credential_login_challenge",
                "请在论坛浏览器完成验证后手动导入 Cookie",
            )
            .into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::LoginUnavailable) => {
            super::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "credential_login_unavailable",
                "论坛登录暂不可用",
            )
            .into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::LoginBusy) => (
            [(axum::http::header::RETRY_AFTER, "30")],
            super::api_error(
                StatusCode::TOO_MANY_REQUESTS,
                "credential_login_busy",
                "论坛登录繁忙，请稍后重试",
            ),
        )
            .into_response(),
        ApplicationError::AdminCredential(AdminCredentialError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::AdminCredential(AdminCredentialError::Conflict)
        | ApplicationError::AdminCredential(AdminCredentialError::CommandConflict) => {
            super::api_error(
                StatusCode::CONFLICT,
                "credential_conflict",
                "同类 active Cookie、凭据标签、指纹或命令负载与当前记录冲突",
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
        ApplicationError::AdminCredential(AdminCredentialError::CheckBusy) => (
            [(axum::http::header::RETRY_AFTER, "2")],
            super::api_error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Cookie 校验繁忙，请稍后重试",
            ),
        )
            .into_response(),
        ApplicationError::AdminCredential(AdminCredentialError::CheckUnavailable) => {
            super::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "credential_check_unavailable",
                "Cookie 校验暂不可用",
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
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_credentials",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}
