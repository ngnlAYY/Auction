//! Admin 结算与 recovery 安全报告读取处理器；报告校验与契约映射按报告类型归属子模块。

mod recovery;
mod settlement;

use std::net::SocketAddr;

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, SafeErrorKind};
use auction_contracts::ApiResponse;
use axum::Json;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::diagnostics;
use crate::errors::{
    CAPABILITY_NOT_PROVIDED, DATABASE_NOT_CONFIGURED, api_error, feature_unavailable,
    not_found_error,
};
use crate::query::parse_query;
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;
use recovery::{map_recovery_report, valid_recovery_report};
use settlement::{map_settlement_report, valid_settlement_report};

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
        return feature_unavailable(DATABASE_NOT_CONFIGURED);
    };
    let report =
        match reader.settlement_report(auction_domain::AuctionId::from_uuid(auction_uuid)).await {
            Ok(Some(report)) => report,
            Ok(None) => return not_found_error().into_response(),
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                return feature_unavailable(CAPABILITY_NOT_PROVIDED);
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

/// 读取 recovery 安全投影；内部 claim/lease 字段永不进入 HTTP。
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
    let include_send_attribution = match recovery_report_projection(&uri) {
        Ok(include) => include,
        Err(response) => return *response,
    };
    let Ok(auction_uuid) = Uuid::parse_str(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    if auction_uuid.is_nil() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    let Some(reader) = state.recovery_report() else {
        return feature_unavailable(DATABASE_NOT_CONFIGURED);
    };
    let report =
        match reader.recovery_report(auction_domain::AuctionId::from_uuid(auction_uuid)).await {
            Ok(Some(report)) => report,
            Ok(None) => return not_found_error().into_response(),
            Err(ApplicationError::CapabilityUnavailable(_)) => {
                return feature_unavailable(CAPABILITY_NOT_PROVIDED);
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
    let report = match map_recovery_report(report, include_send_attribution) {
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

/// 恢复报告的响应投影：缺省是已发布的字段集合；只接受可选的 `include=send_attribution` 追加
/// 发送归属原因。未登记的键、重复出现或其他取值都是 400 `invalid_query`，不静默退回缺省投影。
fn recovery_report_projection(uri: &Uri) -> Result<bool, Box<Response>> {
    let invalid = || {
        Box::new(
            api_error(StatusCode::BAD_REQUEST, "invalid_query", "恢复报告查询参数无效")
                .into_response(),
        )
    };
    let mut query = parse_query(uri, &["include"]).map_err(|()| invalid())?;
    match query.remove("include").as_deref() {
        None => Ok(false),
        Some("send_attribution") => Ok(true),
        Some(_) => Err(invalid()),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Uri;

    use super::recovery_report_projection;

    /// 缺省（含空查询）是已发布投影；只有精确的 `include=send_attribution` 追加发送归属原因，其他
    /// 键、重复或取值都拒绝。
    #[test]
    fn recovery_report_accepts_only_the_send_attribution_include() {
        let parse =
            |query: &str| recovery_report_projection(&query.parse::<Uri>().expect("合法 URI")).ok();
        assert_eq!(parse("/report"), Some(false));
        assert_eq!(parse("/report?"), Some(false));
        assert_eq!(parse("/report?include=send_attribution"), Some(true));
        for rejected in [
            "/report?include=",
            "/report?include=Send_Attribution",
            "/report?include=send_attribution,actions",
            "/report?include=send_attribution&include=send_attribution",
            "/report?page=1",
        ] {
            assert_eq!(parse(rejected), None, "{rejected}");
        }
    }
}
