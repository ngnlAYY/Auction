use auction_application::{ApplicationError, CommandLedgerError, CompetitionTransactionError};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub(crate) fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 拍卖列表仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

pub(crate) fn create_feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 创建拍卖仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

pub(crate) fn map_create_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Domain(auction_domain::DomainError::InvalidSchedule(_)) => {
            super::api_error(
                StatusCode::BAD_REQUEST,
                "invalid_auction_create",
                "后台拍卖创建参数无效",
            )
            .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict) => {
            super::api_error(StatusCode::CONFLICT, "auction_conflict", "拍卖创建与现有状态冲突")
                .into_response()
        }
        ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CapabilityUnavailable(_) => create_feature_unavailable(),
        ApplicationError::Serialization(_) => super::internal_error(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin 创建拍卖暂不可用",
        )
        .into_response(),
    }
}

pub(crate) fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin 拍卖列表暂不可用",
        )
        .into_response(),
    }
}
