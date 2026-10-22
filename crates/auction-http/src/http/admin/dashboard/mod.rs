//! Admin 健康 dashboard 的 HTTP 安全投影。
//! 根模块只负责认证后的请求编排和稳定路由入口；各业务 section 的安全聚合与响应字段映射
//! 分别由子模块拥有。owner 未装配或读取失败时明确返回 unknown，避免用缓存或占位数据误导运维。

mod mapping;
mod sections;

use std::net::SocketAddr;

use auction_application::{WorkerControlCondition, WorkerControlStatus};
use auction_contracts::{AdminDashboardSectionResponse, ApiResponse};
use auction_observability::HealthError;
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::AdminApiState;

pub(super) struct AdminDashboardSections {
    pub(super) mail: AdminDashboardSectionResponse,
    pub(super) realtime: AdminDashboardSectionResponse,
    pub(super) credentials: AdminDashboardSectionResponse,
    pub(super) delivery: AdminDashboardSectionResponse,
    pub(super) fulfillment: AdminDashboardSectionResponse,
    pub(super) recovery: AdminDashboardSectionResponse,
    pub(super) alerting: AdminDashboardSectionResponse,
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
    let worker = match state.worker_control() {
        Some(port) => match port.status().await {
            Ok(status) => status,
            Err(_) => WorkerControlStatus::unavailable(WorkerControlCondition::Unavailable),
        },
        None => WorkerControlStatus::unavailable(WorkerControlCondition::Unavailable),
    };
    let mail = match state.admin_mail() {
        Some(port) => sections::load_mail_section(port, state.dashboard_mailbox_keys())
            .await
            .unwrap_or_else(|_| sections::unknown_section()),
        None => sections::unknown_section(),
    };
    let realtime = match state.realtime_admin() {
        Some(port) => sections::load_realtime_section(port)
            .await
            .unwrap_or_else(|_| sections::unknown_section()),
        None => sections::unknown_section(),
    };
    let owner_snapshot = match state.operations_alert_source() {
        Some(port) => port.snapshot(chrono::Utc::now()).await.ok(),
        None => None,
    };
    let fulfillment = owner_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.fulfillment.map(sections::map_fulfillment))
        .unwrap_or_else(sections::unknown_section);
    let delivery = owner_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.delivery.map(sections::map_delivery))
        .unwrap_or_else(sections::unknown_section);
    let recovery = owner_snapshot
        .and_then(|snapshot| snapshot.recovery.map(sections::map_recovery))
        .unwrap_or_else(sections::unknown_section);
    let alerting = match state.operations_alert() {
        Some(port) => match port.health().await {
            Ok(health) => sections::map_alerting(health),
            Err(_) => sections::unknown_section_with_code("source_unavailable"),
        },
        None => sections::unknown_section_with_code("source_unavailable"),
    };
    let credentials = match state.credential_health() {
        Some(port) => match port.health().await {
            Ok(credentials) => {
                sections::map_credentials(credentials, &snapshot.process, state.dashboard_worlds())
            }
            Err(_) => sections::unknown_section_with_code("source_unavailable"),
        },
        None => sections::unknown_section_with_code("source_unavailable"),
    };
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
    match error {
        HealthError::InvalidCommand | HealthError::StaleObservation => super::internal_error(),
        HealthError::InvalidData | HealthError::Storage { .. } => {
            unavailable("健康 dashboard 暂不可用")
        }
    }
}
