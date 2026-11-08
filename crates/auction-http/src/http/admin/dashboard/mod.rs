//! Admin 健康 dashboard 的 HTTP 安全投影。
//! 根模块只负责认证后的请求编排和稳定路由入口；各业务 section 的安全聚合与响应字段映射
//! 分别由子模块拥有。owner 未装配或读取失败时明确返回 unknown，避免用缓存或占位数据误导运维。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, SafeErrorKind};

use crate::diagnostics;

mod mapping;
mod sections;

use std::net::SocketAddr;

use auction_application::operations::{WorkerControlCondition, WorkerControlStatus};
use auction_contracts::{AdminDashboardSectionResponse, ApiResponse};
use auction_observability::HealthError;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::AdminApiState;

pub(crate) struct AdminDashboardSections {
    pub(crate) mail: AdminDashboardSectionResponse,
    pub(crate) realtime: AdminDashboardSectionResponse,
    pub(crate) credentials: AdminDashboardSectionResponse,
    pub(crate) delivery: AdminDashboardSectionResponse,
    pub(crate) fulfillment: AdminDashboardSectionResponse,
    pub(crate) recovery: AdminDashboardSectionResponse,
    pub(crate) alerting: AdminDashboardSectionResponse,
    pub(crate) reconciliation: AdminDashboardSectionResponse,
}

/// 读取已认证 Admin dashboard。
pub(crate) async fn get(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "管理概览不接受查询参数",
        )
        .into_response();
    }
    let Some(reader) = state.health() else {
        return unavailable("健康 dashboard 需要已配置的数据库");
    };
    let snapshot = match reader.snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => return map_error(error),
    };
    // 基础健康读取成功后，各 owner 的只读 section 彼此独立；同一请求内并发等待，
    // 避免依赖延迟相加耗尽总预算。无独立 spawn，取消请求仍会一起释放这些读取。
    let worker = async {
        match state.worker_control() {
            Some(port) => match port.status().await {
                Ok(status) => status,
                Err(diagnostic_cause) => {
                    diagnostics::warn(
                        "admin_dashboard",
                        "read_worker",
                        DiagnosticError::diagnostic_kind(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    WorkerControlStatus::unavailable(WorkerControlCondition::Unavailable)
                }
            },
            None => WorkerControlStatus::unavailable(WorkerControlCondition::Unavailable),
        }
    };
    let mail = async {
        match state.admin_mail() {
            Some(port) => sections::load_mail_section(port, state.dashboard_mailbox_keys())
                .await
                .unwrap_or_else(|_| sections::unknown_section()),
            None => sections::unknown_section(),
        }
    };
    let realtime = async {
        match state.realtime_admin() {
            Some(port) => sections::load_realtime_section(port)
                .await
                .unwrap_or_else(|_| sections::unknown_section()),
            None => sections::unknown_section(),
        }
    };
    let owner_snapshot = async {
        match state.operations_alert_source() {
            Some(port) => port
                .snapshot(chrono::Utc::now())
                .await
                .inspect_err(|cause| {
                    diagnostics::warn(
                        "admin_dashboard",
                        "read_operations_snapshot",
                        cause.diagnostic_kind(),
                        DiagnosticContext::default(),
                    );
                })
                .ok(),
            None => None,
        }
    };
    let alerting = async {
        match state.operations_alert() {
            Some(port) => match port.health().await {
                Ok(health) => sections::map_alerting(health),
                Err(diagnostic_cause) => {
                    diagnostics::warn(
                        "admin_dashboard",
                        "read_alert_health",
                        DiagnosticError::diagnostic_kind(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    sections::unknown_section_with_code("source_unavailable")
                }
            },
            None => sections::unknown_section_with_code("source_unavailable"),
        }
    };
    let credentials = async {
        match state.credential_health() {
            Some(port) => match port.health().await {
                Ok(credentials) if credentials.validate() => sections::map_credentials(
                    credentials,
                    &snapshot.process,
                    state.dashboard_worlds(),
                ),
                diagnostic_outcome => {
                    let (phase, safe_kind) = match &diagnostic_outcome {
                        Err(cause) => ("read_credential_snapshot", cause.diagnostic_kind()),
                        Ok(_) => {
                            ("validate_credential_snapshot", SafeErrorKind::DEPENDENCY_MISMATCH)
                        }
                    };
                    diagnostics::warn(
                        "admin_dashboard",
                        phase,
                        safe_kind,
                        DiagnosticContext::default(),
                    );
                    drop(diagnostic_outcome);
                    sections::unknown_section_with_code("source_unavailable")
                }
            },
            None => sections::unknown_section_with_code("source_unavailable"),
        }
    };
    let (worker, mail, realtime, owner_snapshot, alerting, credentials) =
        tokio::join!(worker, mail, realtime, owner_snapshot, alerting, credentials);
    let alert_policy = state.operations_alert_policy();
    let fulfillment = owner_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot.fulfillment.map(|value| sections::map_fulfillment(value, alert_policy))
        })
        .unwrap_or_else(sections::unknown_section);
    let delivery = owner_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot.delivery.map(|value| sections::map_delivery(value, alert_policy))
        })
        .unwrap_or_else(sections::unknown_section);
    let recovery = owner_snapshot
        .as_ref()
        .and_then(|snapshot| {
            snapshot.recovery.clone().map(|value| sections::map_recovery(value, alert_policy))
        })
        .unwrap_or_else(sections::unknown_section);
    let reconciliation = owner_snapshot
        .and_then(|snapshot| snapshot.reconciliation.map(sections::map_reconciliation))
        .unwrap_or_else(sections::unknown_section);
    let response = mapping::map_dashboard(
        snapshot,
        worker,
        AdminDashboardSections {
            mail,
            realtime,
            credentials,
            delivery,
            fulfillment,
            recovery,
            alerting,
            reconciliation,
        },
    );
    (StatusCode::OK, super::no_store_headers(), Json(ApiResponse::ok("管理概览已加载", response)))
        .into_response()
}

fn unavailable(message: &'static str) -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", message)
        .into_response()
}

fn map_error(error: HealthError) -> Response {
    let diagnostic_kind = diagnostics::health_kind(&error);
    let body = || match error {
        HealthError::InvalidCommand | HealthError::StaleObservation => super::internal_error(),
        HealthError::InvalidData | HealthError::Storage { .. } => {
            unavailable("健康 dashboard 暂不可用")
        }
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_dashboard",
        "respond_health_failure",
        diagnostic_kind,
        response,
    )
}
