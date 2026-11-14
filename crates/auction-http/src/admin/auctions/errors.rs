//! Admin 拍卖处理器共用的错误映射。

use auction_application::core::diagnostics::DiagnosticError;
use auction_application::core::{
    ApplicationError, CommandLedgerError, CompetitionTransactionError,
};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::diagnostics;
use crate::errors::{
    CAPABILITY_NOT_PROVIDED, DATABASE_NOT_CONFIGURED, api_error, storage_unavailable,
};

pub(crate) fn invalid_create_command() -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_auction_command", "后台拍卖创建参数无效")
        .into_response()
}

pub(crate) fn feature_unavailable() -> Response {
    crate::errors::feature_unavailable(DATABASE_NOT_CONFIGURED)
}

pub(crate) fn map_create_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::Domain(auction_domain::DomainError::InvalidSchedule(_)) => {
            invalid_create_command()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "not_found", "拍卖或关联资源不存在").into_response()
        }
        ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::Conflict | CompetitionTransactionError::InvalidState,
        ) => api_error(StatusCode::CONFLICT, "invalid_auction_state", "拍卖状态不允许当前命令")
            .into_response(),
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::StaleState) => {
            api_error(StatusCode::CONFLICT, "stale_auction_state", "拍卖版本已变化").into_response()
        }
        ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CapabilityUnavailable(_) => {
            crate::errors::feature_unavailable(CAPABILITY_NOT_PROVIDED)
        }
        _ => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
                .into_response()
        }
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_auctions",
        "respond_create_failure",
        diagnostic_kind,
        response,
    )
}

pub(crate) fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::CapabilityUnavailable(_) => {
            crate::errors::feature_unavailable(CAPABILITY_NOT_PROVIDED)
        }
        _ => storage_unavailable("Admin 拍卖列表暂不可用"),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_auctions",
        "respond_lifecycle_failure",
        diagnostic_kind,
        response,
    )
}
