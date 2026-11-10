use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

use auction_application::admin::{AdminMailError, AdminMailTransactionError};
use auction_application::core::ApplicationError;
use auction_contracts::ApiErrorResponseWithData;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub(crate) fn feature_unavailable() -> Response {
    super::super::feature_unavailable("Admin 邮件状态与入站审查仅支持 PostgreSQL 完整档")
}

pub(crate) fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminMail(AdminMailError::InvalidRequest)
        | ApplicationError::AdminMail(AdminMailError::InvalidCommand) => {
            super::api_error(StatusCode::BAD_REQUEST, "invalid_mail_request", "邮件参数无效")
                .into_response()
        }
        ApplicationError::AdminMail(AdminMailError::CommandConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::AdminMail(AdminMailError::InvalidProjection)
        | ApplicationError::AdminMail(AdminMailError::StoredResultCorrupt) => {
            super::internal_error()
        }
        ApplicationError::AdminMailTransaction(AdminMailTransactionError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::AdminMailTransaction(AdminMailTransactionError::StateConflict {
            current_state_version,
        }) => (
            StatusCode::CONFLICT,
            axum::Json(ApiErrorResponseWithData {
                code: "stale_state_version".to_owned(),
                message: "邮件轮询状态版本已变化".to_owned(),
                data: serde_json::json!({ "current_state_version": current_state_version }),
                request_id: None,
            }),
        )
            .into_response(),
        ApplicationError::AdminMailTransaction(AdminMailTransactionError::Conflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::Serialization(_) => super::internal_error(),
        _ => super::super::storage_unavailable("邮件服务暂不可用"),
    };
    let response = body();
    diagnostics::mapped_response("admin_mail", "respond_command_failure", diagnostic_kind, response)
}
