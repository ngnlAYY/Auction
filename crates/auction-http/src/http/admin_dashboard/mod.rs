//! Admin 健康 dashboard 的 HTTP 安全投影。
//!
//! 当前先接入 service_heartbeats 的 process/health section。其他 owner 的 section 明确返回
//! `unknown`，避免用占位数据误导运维；后续 mail、realtime、delivery 等 owner 只需扩展
//! `HealthReader` 的聚合投影，不改变路由和会话边界。

use std::net::SocketAddr;

use auction_contracts::{
    AdminDashboardHealthResponse, AdminDashboardResponse, AdminDashboardSectionResponse,
    AdminDashboardWorkerResponse, ApiResponse,
};
use auction_observability::{
    AdminDashboardSnapshot, DashboardSection, DashboardStatus, HealthError, HealthSummary,
    ProcessSnapshot, ProcessState, ServiceName, TracingHealth,
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::AdminApiState;

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
    let response = map_dashboard(snapshot);
    (StatusCode::OK, super::no_store_headers(), Json(ApiResponse::ok("管理概览已加载", response)))
        .into_response()
}

fn map_dashboard(snapshot: AdminDashboardSnapshot) -> AdminDashboardResponse {
    AdminDashboardResponse {
        health: map_health(snapshot.health),
        // Worker control socket 尚未迁移；不可用状态必须清空其他字段，不能伪造 ready。
        worker: AdminDashboardWorkerResponse {
            condition: "unavailable".to_owned(),
            state: String::new(),
            ready: false,
            version: String::new(),
            mail_pollers: 0,
        },
        process: map_process_section(snapshot.process),
        mail: unknown_section(),
        realtime: unknown_section(),
        credentials: unknown_section(),
        delivery: unknown_section(),
        fulfillment: unknown_section(),
        recovery: unknown_section(),
        alerting: unknown_section(),
    }
}

fn map_health(summary: HealthSummary) -> AdminDashboardHealthResponse {
    AdminDashboardHealthResponse {
        status: summary.status.as_str().to_owned(),
        readiness: summary.readiness,
        audit_failure_count: summary.audit_failure_count,
        missing_services: summary
            .missing_services
            .into_iter()
            .map(|service| service_role_name(service).to_owned())
            .collect(),
    }
}

fn map_process_section(
    section: DashboardSection<ProcessSnapshot>,
) -> AdminDashboardSectionResponse {
    let data = section.data.map(map_process_data);
    AdminDashboardSectionResponse {
        status: section.status.as_str().to_owned(),
        observed_at: section.observed_at.map(super::timestamp),
        error_code: section.error_code.map(ToOwned::to_owned),
        data,
    }
}

fn map_process_data(snapshot: ProcessSnapshot) -> serde_json::Value {
    json!({
        "processes": snapshot.processes.into_iter().map(map_process).collect::<Vec<_>>(),
        "summary": map_health(snapshot.summary),
        "missing_services": snapshot.missing_services.into_iter()
            .map(|service| service_role_name(service).to_owned())
            .collect::<Vec<_>>(),
        "observed_at": snapshot.observed_at.map(super::timestamp),
    })
}

fn map_process(process: ProcessState) -> serde_json::Value {
    json!({
        "service_name": process.service_name.as_str(),
        "process_role": process.process_role.as_str(),
        "instance_id": process.instance_id,
        "version": process.version,
        "started_at": super::timestamp(process.started_at),
        "status": process.status.as_str(),
        "readiness": process.readiness,
        "audit_failure_count": process.audit_failure_count,
        "tracing": map_tracing(process.tracing),
        "observed_at": super::timestamp(process.observed_at),
        "expires_at": super::timestamp(process.expires_at),
    })
}

fn map_tracing(health: TracingHealth) -> serde_json::Value {
    json!({
        "status": health.status.as_str(),
        "dropped_span_count": health.dropped_span_count,
        "export_failure_count": health.export_failure_count,
        "last_success_at": health.last_success_at.map(super::timestamp),
        "last_failure_at": health.last_failure_at.map(super::timestamp),
        "last_error_code": health.last_error_code,
    })
}

fn unknown_section() -> AdminDashboardSectionResponse {
    AdminDashboardSectionResponse {
        status: DashboardStatus::Unknown.as_str().to_owned(),
        observed_at: None,
        error_code: Some("source_unavailable".to_owned()),
        data: None,
    }
}

fn service_role_name(service: ServiceName) -> &'static str {
    match service {
        ServiceName::Public => "public",
        ServiceName::Admin => "admin",
        ServiceName::Worker => "worker",
    }
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
