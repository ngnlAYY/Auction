//! 会话、生命周期、竞价和恢复命令的统一 HTTP 错误映射。

use super::*;

pub(crate) fn map_login_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Session(SessionError::InvalidLoginKey) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_key", "登录凭据格式无效").into_response()
        }
        ApplicationError::Session(SessionError::InvalidCredentials)
        | ApplicationError::Session(SessionError::InvalidIdentity) => {
            api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                .into_response()
        }
        ApplicationError::Session(SessionError::AccountInactive { public_reason }) => {
            let Some(public_reason) = public_reason else {
                return api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                    .into_response();
            };
            (
                StatusCode::FORBIDDEN,
                Json(ApiErrorResponseWithData {
                    code: "account_blacklisted".to_owned(),
                    message: "账号已被封禁".to_owned(),
                    data: PublicAccountBlockedResponse { public_reason },
                    request_id: None,
                }),
            )
                .into_response()
        }
        ApplicationError::Session(SessionError::SessionLimit) => {
            api_error(StatusCode::CONFLICT, "session_limit", "并发会话数量已达上限").into_response()
        }
        _ => session_unavailable(),
    }
}

pub(crate) fn map_admin_session_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Session(SessionError::InvalidCredentials)
        | ApplicationError::Session(SessionError::InvalidIdentity) => {
            api_error(StatusCode::UNAUTHORIZED, "invalid_credentials", "登录凭据无效")
                .into_response()
        }
        ApplicationError::Session(SessionError::Unauthenticated) => authentication_error(),
        ApplicationError::Session(SessionError::InvalidConfiguration)
        | ApplicationError::Session(SessionError::TokenGeneration) => session_unavailable(),
        _ => session_unavailable(),
    }
}

pub(crate) fn map_admin_lifecycle_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Domain(DomainError::InvalidTransition { .. })
        | ApplicationError::Domain(DomainError::InvalidSchedule(_))
        | ApplicationError::Domain(DomainError::CancellationCutoff) => {
            api_error(StatusCode::CONFLICT, "invalid_lifecycle", "拍卖当前不允许该生命周期操作")
                .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            not_found_error().into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::StaleState) => {
            api_error(
                StatusCode::CONFLICT,
                "state_version_conflict",
                "拍卖状态已变化，请刷新后重试",
            )
            .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::Serialization(_) => internal_error(),
        _ => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖生命周期暂不可用",
        )
        .into_response(),
    }
}

pub(crate) fn is_unauthenticated(error: &ApplicationError) -> bool {
    matches!(error, ApplicationError::Session(SessionError::Unauthenticated))
}

pub(crate) fn map_competition_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminLotUpdate(AdminLotUpdateError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_lot_command", "后台拍品修改参数无效")
                .into_response()
        }
        ApplicationError::Domain(DomainError::InvalidAmount { .. }) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_amount", "出价金额无效").into_response()
        }
        ApplicationError::Domain(DomainError::BidTooLow { .. }) => {
            api_error(StatusCode::CONFLICT, "bid_too_low", "出价未达到当前最低价").into_response()
        }
        ApplicationError::Domain(DomainError::InvalidProxyMaximum) => {
            api_error(StatusCode::CONFLICT, "invalid_proxy_maximum", "代理上限当前不可调整")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxyDisabled) => {
            api_error(StatusCode::CONFLICT, "proxy_disabled", "当前拍卖未启用代理竞价")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxyOutbidLimit) => {
            api_error(StatusCode::CONFLICT, "proxy_outbid_limit", "代理重复进场次数已用尽")
                .into_response()
        }
        ApplicationError::Domain(DomainError::ProxySlowFinal) => {
            api_error(StatusCode::CONFLICT, "proxy_slow_final", "最终阶段不允许使用慢速代理")
                .into_response()
        }
        ApplicationError::Domain(DomainError::InvalidProxyTransition { .. }) => {
            api_error(StatusCode::CONFLICT, "invalid_proxy_state", "代理授权当前不可调整")
                .into_response()
        }
        ApplicationError::Domain(DomainError::AuctionClosed)
        | ApplicationError::Domain(DomainError::InvalidBidState(_)) => {
            api_error(StatusCode::CONFLICT, "auction_not_open", "当前拍卖不接受竞价")
                .into_response()
        }
        ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            not_found_error().into_response()
        }
        ApplicationError::Serialization(_) => internal_error(),
        ApplicationError::CapabilityUnavailable(_)
        | ApplicationError::FulfillmentEvidenceNotObserved
        | ApplicationError::Domain(_)
        | ApplicationError::Repository(_)
        | ApplicationError::CompetitionTransaction(_)
        | ApplicationError::TransactionPolicy(_)
        | ApplicationError::CommandLedger(_)
        | ApplicationError::RecoveryCommand(_)
        | ApplicationError::RealtimeAdmin(_)
        | ApplicationError::SettlementTransaction(_)
        | ApplicationError::AdminRules(_)
        | ApplicationError::AdminRulesTransaction(_)
        | ApplicationError::AdminIntake(_)
        | ApplicationError::AdminForumPreview(_)
        | ApplicationError::AdminAudit(_)
        | ApplicationError::AdminCredential(_)
        | ApplicationError::AdminMail(_)
        | ApplicationError::AdminMailTransaction(_)
        | ApplicationError::AdminRuntime(_)
        | ApplicationError::AdminRuntimeFile(_)
        | ApplicationError::AdminRuntimeCommand(_)
        | ApplicationError::AdminUser(_)
        | ApplicationError::MailProcessing(_)
        | ApplicationError::ForumRemote(_)
        | ApplicationError::Session(_)
        | ApplicationError::OperationsAlert(_) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "竞价服务暂不可用")
                .into_response()
        }
    }
}

pub(crate) fn map_recovery_command_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminLotUpdate(AdminLotUpdateError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_lot_command", "后台拍品修改参数无效")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "not_found", "恢复记录不存在").into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::StaleState) => {
            api_error(StatusCode::CONFLICT, "stale_recovery_state", "恢复状态版本已变化")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::NotConfirmable) => {
            api_error(StatusCode::CONFLICT, "recovery_not_confirmable", "恢复状态不允许当前确认")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::EvidenceUnresolved) => {
            api_error(StatusCode::CONFLICT, "recovery_evidence_unresolved", "恢复证据尚未解决")
                .into_response()
        }
        ApplicationError::RecoveryCommand(RecoveryCommandError::CommandConflict) => {
            api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复确认仅支持 PostgreSQL 完整档",
        )
        .into_response(),
        ApplicationError::FulfillmentEvidenceNotObserved
        | ApplicationError::RecoveryCommand(_)
        | ApplicationError::Serialization(_)
        | ApplicationError::Repository(_)
        | ApplicationError::TransactionPolicy(_)
        | ApplicationError::CompetitionTransaction(_)
        | ApplicationError::SettlementTransaction(_)
        | ApplicationError::CommandLedger(_)
        | ApplicationError::Domain(_)
        | ApplicationError::RealtimeAdmin(_)
        | ApplicationError::AdminRules(_)
        | ApplicationError::AdminRulesTransaction(_)
        | ApplicationError::AdminIntake(_)
        | ApplicationError::AdminForumPreview(_)
        | ApplicationError::AdminAudit(_)
        | ApplicationError::AdminCredential(_)
        | ApplicationError::AdminMail(_)
        | ApplicationError::AdminMailTransaction(_)
        | ApplicationError::AdminRuntime(_)
        | ApplicationError::AdminRuntimeFile(_)
        | ApplicationError::AdminRuntimeCommand(_)
        | ApplicationError::AdminUser(_)
        | ApplicationError::MailProcessing(_)
        | ApplicationError::ForumRemote(_)
        | ApplicationError::Session(_)
        | ApplicationError::OperationsAlert(_) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "恢复确认服务暂不可用",
        )
        .into_response(),
    }
}

pub(crate) fn authentication_error() -> Response {
    api_error(StatusCode::UNAUTHORIZED, "unauthenticated", "会话无效或已过期").into_response()
}

pub(crate) fn session_unavailable() -> Response {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "会话服务暂不可用")
        .into_response()
}

pub(crate) fn internal_error() -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "服务内部错误").into_response()
}
