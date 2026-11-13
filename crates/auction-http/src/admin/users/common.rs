//! Admin 用户处理器共用的请求边界与错误映射。

use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

pub(super) use super::super::parse_canonical_uuid;
use auction_application::admin::AdminUserError;
use auction_application::core::ApplicationError;
use auction_contracts::ApiErrorResponseWithData;
use axum::{
    Json,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};

/// 校验 Admin 用户写入共同使用的查询字符串边界。
pub(super) fn mutation_boundary(uri: &Uri) -> Option<Response> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "用户命令不接受查询参数")
                .into_response(),
        );
    }
    None
}

/// 用户管理的数据库档位缺失按依赖不可用处理：保留 503 `feature_unavailable`，
/// 不能复用路由层的 501 `feature_not_supported` 外壳。
pub(super) fn feature_unavailable() -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "feature_unavailable", "用户管理服务暂不可用")
        .into_response()
}

pub(super) fn invalid_query() -> Response {
    super::super::invalid_query("查询参数无效")
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

pub(super) fn invalid_user_command() -> Response {
    invalid_request("invalid_user_command", "用户管理参数无效")
}

pub(super) fn invalid_rename_command() -> Response {
    invalid_request("invalid_rename_command", "改名回滚参数无效")
}

pub(super) fn invalid_request(code: &'static str, message: &'static str) -> Response {
    super::api_error(StatusCode::BAD_REQUEST, code, message).into_response()
}

pub(super) fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || {
        match error {
            ApplicationError::AdminUser(AdminUserError::InvalidRequest) => invalid_user_command(),
            ApplicationError::AdminUser(AdminUserError::NotFound) => {
                super::api_error(StatusCode::NOT_FOUND, "not_found", "用户不存在").into_response()
            }
            ApplicationError::AdminUser(AdminUserError::Conflict) => {
                super::api_error(StatusCode::CONFLICT, "user_conflict", "用户与现有记录冲突")
                    .into_response()
            }
            ApplicationError::AdminUser(AdminUserError::CommandConflict) => {
                super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                    .into_response()
            }
            ApplicationError::AdminUser(AdminUserError::VersionConflict { current_version }) => {
                version_conflict("state_conflict", current_version)
            }
            ApplicationError::AdminUser(AdminUserError::RenameNotRevertable) => {
                super::api_error(StatusCode::CONFLICT, "rename_not_revertable", "改名记录不能回滚")
                    .into_response()
            }
            ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
            // 密钥不可用、结果损坏及其余依赖错误统一返回依赖不可用。
            _ => dependency_unavailable(),
        }
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_users",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}

/// 改名回滚沿用旧版独立的业务错误命名，不能与普通用户命令合并。
pub(super) fn map_revert_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminUser(AdminUserError::InvalidRequest) => invalid_rename_command(),
        ApplicationError::AdminUser(AdminUserError::NotFound) => {
            super::api_error(StatusCode::NOT_FOUND, "not_found", "改名记录不存在").into_response()
        }
        ApplicationError::AdminUser(AdminUserError::Conflict) => {
            super::api_error(StatusCode::CONFLICT, "rename_name_taken", "原用户名已被占用")
                .into_response()
        }
        ApplicationError::AdminUser(AdminUserError::CommandConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::AdminUser(AdminUserError::VersionConflict { current_version }) => {
            version_conflict("stale_user_state", current_version)
        }
        ApplicationError::AdminUser(AdminUserError::RenameNotRevertable) => {
            super::api_error(StatusCode::CONFLICT, "rename_not_revertable", "改名记录不能回滚")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => dependency_unavailable(),
    };
    let response = body();
    diagnostics::mapped_response("admin_users", "respond_revert_failure", diagnostic_kind, response)
}

/// 用户列表只把 owner 的请求拒绝映射为查询错误，其余失败保持依赖错误外壳。
pub(super) fn map_list_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminUser(AdminUserError::InvalidRequest) => invalid_query(),
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => dependency_unavailable(),
    };
    let response = body();
    diagnostics::mapped_response("admin_users", "respond_list_failure", diagnostic_kind, response)
}

fn version_conflict(code: &'static str, current_version: i64) -> Response {
    (
        StatusCode::CONFLICT,
        Json(ApiErrorResponseWithData {
            code: code.to_owned(),
            message: "用户版本已变化".to_owned(),
            data: serde_json::json!({ "current_state_version": current_version }),
            request_id: None,
        }),
    )
        .into_response()
}

fn dependency_unavailable() -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
        .into_response()
}
