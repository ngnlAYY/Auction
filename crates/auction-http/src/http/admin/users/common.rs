use auction_application::{AdminUserError, ApplicationError};
use axum::{
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

/// 校验 Admin 用户写入共同使用的 CSRF、查询字符串和方法边界。
pub(super) fn mutation_boundary(headers: &HeaderMap, uri: &Uri) -> Option<Response> {
    let Some(raw_csrf) = super::read_admin_cookie(headers, super::ADMIN_CSRF_COOKIE) else {
        return Some(super::authentication_error());
    };
    if !super::valid_admin_csrf_header(headers, &raw_csrf) {
        return Some(
            super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
                .into_response(),
        );
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "用户命令不接受查询参数")
                .into_response(),
        );
    }
    None
}

pub(super) fn feature_unavailable() -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "feature_unavailable", "用户管理服务暂不可用")
        .into_response()
}

pub(super) fn invalid_query() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response()
}

pub(super) fn invalid_json() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效").into_response()
}

pub(super) fn invalid_command_id() -> Response {
    invalid_request("invalid_command_id", "命令 ID 无效")
}

pub(super) fn invalid_user_id() -> Response {
    invalid_request("invalid_user_id", "用户 ID 无效")
}

pub(super) fn invalid_request(code: &'static str, message: &'static str) -> Response {
    super::api_error(StatusCode::BAD_REQUEST, code, message).into_response()
}

pub(super) fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminUser(AdminUserError::InvalidRequest) => {
            invalid_request("invalid_user_request", "用户管理参数无效")
        }
        ApplicationError::AdminUser(AdminUserError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::AdminUser(AdminUserError::Conflict)
        | ApplicationError::AdminUser(AdminUserError::CommandConflict) => super::api_error(
            StatusCode::CONFLICT,
            "command_conflict",
            "用户状态或命令负载与当前记录冲突",
        )
        .into_response(),
        ApplicationError::AdminUser(AdminUserError::VersionConflict { .. }) => super::api_error(
            StatusCode::CONFLICT,
            "stale_user_state",
            "用户状态版本已变化，请重新读取后再提交",
        )
        .into_response(),
        ApplicationError::AdminUser(AdminUserError::RenameNotRevertable) => super::api_error(
            StatusCode::CONFLICT,
            "rename_not_revertable",
            "该改名记录当前不可回滚",
        )
        .into_response(),
        ApplicationError::AdminUser(AdminUserError::KeyUnavailable) => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "key_unavailable",
            "登录 key 服务暂不可用",
        )
        .into_response(),
        ApplicationError::AdminUser(AdminUserError::StoredResultCorrupt) => super::internal_error(),
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "用户管理服务暂不可用",
        )
        .into_response(),
    }
}
