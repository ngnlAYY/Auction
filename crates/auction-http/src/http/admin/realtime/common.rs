use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

use std::collections::BTreeMap;

use auction_application::core::ApplicationError;
use auction_application::realtime::{
    MAX_REALTIME_ADMIN_OFFSET, MAX_REALTIME_ADMIN_PAGE_SIZE, RealtimeAdminError,
};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub(crate) fn parse_page(query: &BTreeMap<&str, String>) -> Result<(i64, i64, i64), ()> {
    let page = super::parse_positive_i64(query.get("page"), 1, None)?;
    let page_size =
        super::parse_positive_i64(query.get("page_size"), 50, Some(MAX_REALTIME_ADMIN_PAGE_SIZE))?;
    let offset = page.checked_sub(1).and_then(|value| value.checked_mul(page_size)).ok_or(())?;
    if offset > MAX_REALTIME_ADMIN_OFFSET {
        return Err(());
    }
    Ok((page, page_size, offset))
}

pub(crate) fn map_realtime_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::RealtimeAdmin(RealtimeAdminError::InvalidCommand) => super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_realtime_command",
            "realtime 管理参数无效",
        )
        .into_response(),
        ApplicationError::RealtimeAdmin(RealtimeAdminError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::RealtimeAdmin(RealtimeAdminError::AlreadyResolved) => super::api_error(
            StatusCode::CONFLICT,
            "realtime_failure_resolved",
            "realtime failure 已经解决",
        )
        .into_response(),
        ApplicationError::RealtimeAdmin(RealtimeAdminError::ReplayMismatch) => super::api_error(
            StatusCode::CONFLICT,
            "realtime_replay_mismatch",
            "replay 与原 realtime failure 不匹配",
        )
        .into_response(),
        ApplicationError::RealtimeAdmin(RealtimeAdminError::CommandConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::super::storage_unavailable("realtime 管理服务暂不可用"),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_realtime",
        "respond_read_failure",
        diagnostic_kind,
        response,
    )
}

/// 把内部 typed replay 错误投影回旧 v1 的稳定 HTTP 外壳。
/// 存储层继续区分 resolved、mismatch、命令冲突和 capability；这里只收窄公开 code/status，
/// 不回退命令账本、事务 fence 或错误正文隔离。
pub(crate) fn map_replay_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::RealtimeAdmin(RealtimeAdminError::InvalidCommand) => invalid_replay(),
        ApplicationError::RealtimeAdmin(RealtimeAdminError::NotFound) => {
            super::api_error(StatusCode::NOT_FOUND, "not_found", "实时 failure 不存在")
                .into_response()
        }
        ApplicationError::RealtimeAdmin(
            RealtimeAdminError::AlreadyResolved | RealtimeAdminError::CommandConflict,
        ) => super::api_error(StatusCode::CONFLICT, "conflict", "实时 failure 已解决")
            .into_response(),
        ApplicationError::RealtimeAdmin(RealtimeAdminError::ReplayMismatch) => {
            super::api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "服务暂时不可用")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) | ApplicationError::Repository(_) => {
            replay_unavailable()
        }
        _ => replay_unavailable(),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_realtime",
        "respond_replay_failure",
        diagnostic_kind,
        response,
    )
}

pub(crate) fn invalid_replay() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_replay", "重放参数无效").into_response()
}

pub(crate) fn replay_unavailable() -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
        .into_response()
}

pub(crate) fn feature_unavailable() -> Response {
    super::super::feature_unavailable("realtime Admin 依赖当前数据库档位未提供")
}

pub(crate) fn invalid_query() -> Response {
    super::super::invalid_query("查询参数无效")
}
