//! Admin 结算与 recovery 安全报告读取处理器。

use auction_application::diagnostics::{DiagnosticContext, DiagnosticError, SafeErrorKind};

use crate::diagnostics;

use super::super::*;

/// 读取 PostgreSQL 权威结算报告。
pub(crate) async fn admin_settlement_report(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "结算报告不接受查询参数")
            .into_response();
    }
    let Ok(auction_uuid) = Uuid::parse_str(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    if auction_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    let Some(reader) = state.settlement_report() else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "结算报告仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let report =
        match reader.settlement_report(auction_domain::AuctionId::from_uuid(auction_uuid)).await {
            Ok(Some(report)) => report,
            Ok(None) => return not_found_error().into_response(),
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                return api_error(
                    StatusCode::NOT_IMPLEMENTED,
                    "feature_not_supported",
                    "结算报告仅支持 PostgreSQL 完整档",
                )
                .into_response();
            }
            Err(diagnostic_cause) => {
                diagnostics::warn(
                    "admin_reports",
                    "read_settlement",
                    DiagnosticError::diagnostic_kind(&diagnostic_cause),
                    DiagnosticContext { auction_id: Some(auction_uuid), ..Default::default() },
                );

                // 底层错误可能携带 SQL 或连接信息；日志只记录业务标识与闭集失败分类。

                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "结算报告暂不可用",
                )
                .into_response();
            }
        };
    if !valid_settlement_report(&report, auction_uuid) {
        diagnostics::error(
            "admin_reports",
            "validate_settlement",
            SafeErrorKind::DOMAIN_REJECTED,
            DiagnosticContext { auction_id: Some(auction_uuid), ..Default::default() },
        );
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("结算报告已加载", map_settlement_report(report))),
    )
        .into_response()
}

/// 读取 PostgreSQL 完整档的 recovery 安全投影；内部 claim/lease 字段永不进入 HTTP。
pub(crate) async fn admin_recovery_report(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "恢复报告不接受查询参数")
            .into_response();
    }
    let Ok(auction_uuid) = Uuid::parse_str(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    if auction_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    let Some(reader) = state.recovery_report() else {
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复报告仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let report =
        match reader.recovery_report(auction_domain::AuctionId::from_uuid(auction_uuid)).await {
            Ok(Some(report)) => report,
            Ok(None) => return not_found_error().into_response(),
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                return api_error(
                    StatusCode::NOT_IMPLEMENTED,
                    "feature_not_supported",
                    "恢复报告仅支持 PostgreSQL 完整档",
                )
                .into_response();
            }
            Err(diagnostic_cause) => {
                diagnostics::warn(
                    "admin_reports",
                    "read_recovery",
                    DiagnosticError::diagnostic_kind(&diagnostic_cause),
                    DiagnosticContext::default(),
                );

                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "恢复报告暂不可用",
                )
                .into_response();
            }
        };
    if !valid_recovery_report(&report, auction_uuid) {
        return internal_error();
    }
    let report = match map_recovery_report(report) {
        Ok(report) => report,
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_reports",
                "project_recovery",
                diagnostics::invalid_state(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            // 内部错误码不能进入旧 v1 wire；未知值按依赖不可用处理，且不回显原文。
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "恢复报告暂不可用",
            )
            .into_response();
        }
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("恢复报告已加载", report)))
        .into_response()
}
