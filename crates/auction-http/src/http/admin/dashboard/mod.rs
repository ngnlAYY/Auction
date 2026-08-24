//! Admin 健康 dashboard 的 HTTP 安全投影。
//!
//! 当前接入旧版 dashboard 的 process/health、Worker control、mail、realtime、credentials、
//! delivery、fulfillment、recovery 和 alerting 安全聚合。owner 未装配或读取失败时明确返回
//! `unknown`，避免用缓存或占位数据误导运维。

use std::net::SocketAddr;

use auction_application::{
    AdminCredentialHealthSnapshot, AdminCredentialKind, AdminCredentialWorld, AdminMailPort,
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertHealth,
    OperationsAlertRecoverySnapshot, RealtimeAdminPort, WorkerControlCondition,
    WorkerControlStatus,
};
use auction_contracts::{
    AdminDashboardHealthResponse, AdminDashboardResponse, AdminDashboardSectionResponse,
    AdminDashboardWorkerResponse, ApiResponse,
};
use auction_observability::{
    AdminDashboardSnapshot, DashboardSection, DashboardStatus, HealthError, HealthSummary,
    ProcessRole, ProcessSnapshot, ProcessState, ServiceName, TracingHealth,
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
    let worker = match state.worker_control() {
        Some(port) => match port.status().await {
            Ok(status) => status,
            Err(_) => WorkerControlStatus::unavailable(WorkerControlCondition::Unavailable),
        },
        None => WorkerControlStatus::unavailable(WorkerControlCondition::Unavailable),
    };
    let mail = match state.admin_mail() {
        Some(port) => load_mail_section(port, state.dashboard_mailbox_keys())
            .await
            .unwrap_or_else(|_| unknown_section()),
        None => unknown_section(),
    };
    let realtime = match state.realtime_admin() {
        Some(port) => load_realtime_section(port).await.unwrap_or_else(|_| unknown_section()),
        None => unknown_section(),
    };
    let owner_snapshot = match state.operations_alert_source() {
        Some(port) => port.snapshot(chrono::Utc::now()).await.ok(),
        None => None,
    };
    let fulfillment = owner_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.fulfillment.map(map_fulfillment))
        .unwrap_or_else(unknown_section);
    let delivery = owner_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.delivery.map(map_delivery))
        .unwrap_or_else(unknown_section);
    let recovery = owner_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.recovery.clone().map(map_recovery))
        .unwrap_or_else(unknown_section);
    let alerting = match state.operations_alert() {
        Some(port) => match port.health().await {
            Ok(health) => map_alerting(health),
            Err(_) => unknown_section_with_code("source_unavailable"),
        },
        None => unknown_section_with_code("source_unavailable"),
    };
    let credentials = match state.credential_health() {
        Some(port) => match port.health().await {
            Ok(credentials) => {
                map_credentials(credentials, &snapshot.process, state.dashboard_worlds())
            }
            Err(_) => unknown_section_with_code("source_unavailable"),
        },
        None => unknown_section_with_code("source_unavailable"),
    };
    let response = map_dashboard(
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

struct AdminDashboardSections {
    mail: AdminDashboardSectionResponse,
    realtime: AdminDashboardSectionResponse,
    credentials: AdminDashboardSectionResponse,
    delivery: AdminDashboardSectionResponse,
    fulfillment: AdminDashboardSectionResponse,
    recovery: AdminDashboardSectionResponse,
    alerting: AdminDashboardSectionResponse,
}

fn map_dashboard(
    snapshot: AdminDashboardSnapshot,
    worker: WorkerControlStatus,
    sections: AdminDashboardSections,
) -> AdminDashboardResponse {
    AdminDashboardResponse {
        health: map_health(snapshot.health),
        worker: map_worker(worker),
        process: map_process_section(snapshot.process),
        mail: sections.mail,
        realtime: sections.realtime,
        credentials: sections.credentials,
        delivery: sections.delivery,
        fulfillment: sections.fulfillment,
        recovery: sections.recovery,
        alerting: sections.alerting,
    }
}

fn map_fulfillment(snapshot: OperationsAlertFulfillmentSnapshot) -> AdminDashboardSectionResponse {
    let error_code = if snapshot.manual_reconcile > 0 {
        Some("manual_reconcile")
    } else if snapshot.failed > 0 {
        Some("fulfillment_failed")
    } else if snapshot.blocked > 0 {
        Some("fulfillment_blocked")
    } else if snapshot.pending > 0 {
        Some("fulfillment_backlog")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(super::timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::timestamp(chrono::Utc::now()),
            "pending": snapshot.pending,
            "blocked": snapshot.blocked,
            "failed": snapshot.failed,
            "manual_reconcile": snapshot.manual_reconcile,
            "oldest_backlog_age_seconds": snapshot.oldest_backlog_age_seconds,
        })),
    }
}

fn map_delivery(snapshot: OperationsAlertDeliverySnapshot) -> AdminDashboardSectionResponse {
    let error_code = if snapshot.manual_reconcile > 0 {
        Some("manual_reconcile")
    } else if snapshot.failed > 0 {
        Some("delivery_failed")
    } else if snapshot.pending > 0 {
        Some("delivery_backlog")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(super::timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::timestamp(chrono::Utc::now()),
            "water": {
                "pending": snapshot.pending,
                "failed": snapshot.failed,
                "manual_reconcile": snapshot.manual_reconcile,
                "oldest_pending_age_seconds": snapshot.oldest_pending_age_seconds,
                "oldest_manual_reconcile_age_seconds": snapshot.oldest_manual_reconcile_age_seconds,
                "last_update_at": snapshot.last_update_at.map(super::timestamp),
            },
        })),
    }
}

fn map_recovery(snapshot: OperationsAlertRecoverySnapshot) -> AdminDashboardSectionResponse {
    let error_code = if snapshot.manual_reconcile_count > 0 {
        Some("manual_reconcile")
    } else if snapshot.failed_action_count > 0 {
        Some("recovery_action_failed")
    } else if snapshot.payment_overdue_count > 0 {
        Some("payment_overdue")
    } else if snapshot.relist_pending_count > 0 {
        Some("relist_pending")
    } else if snapshot
        .worlds
        .iter()
        .any(|world| world.balance_status != "ok" || world.evidence_status != "ok")
    {
        Some("recovery_source_degraded")
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(super::timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::timestamp(chrono::Utc::now()),
            "worlds": snapshot.worlds.into_iter().map(|world| json!({
                "world": world.world,
                "balance_credits": world.balance_credits,
                "balance_status": world.balance_status,
                "evidence_status": world.evidence_status,
                "evidence_age_seconds": world.evidence_age_seconds,
                "balance_observed_at": world.balance_observed_at.map(super::timestamp),
                "evidence_last_success_at": world.evidence_last_success_at.map(super::timestamp),
                "updated_at": super::timestamp(world.updated_at),
            })).collect::<Vec<_>>(),
            "payment_overdue_count": snapshot.payment_overdue_count,
            "relist_pending_count": snapshot.relist_pending_count,
            "failed_action_count": snapshot.failed_action_count,
            "manual_reconcile_count": snapshot.manual_reconcile_count,
            "oldest_overdue_age_seconds": snapshot.oldest_overdue_age_seconds,
        })),
    }
}

/// 读取所有邮件轮询器的安全 dashboard 摘要。
///
/// 正文、游标和 metadata 不进入摘要；单个邮箱的入站审查仍通过专用列表端点读取。
async fn load_mail_section(
    port: &dyn AdminMailPort,
    configured_mailbox_keys: &[String],
) -> Result<AdminDashboardSectionResponse, ()> {
    let snapshot = port.dashboard_snapshot().await.map_err(|_| ())?;
    if !snapshot.validate() {
        return Err(());
    }
    let scope_mismatch = !configured_mailbox_keys.is_empty()
        && (snapshot.pollers.len() != configured_mailbox_keys.len()
            || snapshot.pollers.iter().any(|poller| {
                !configured_mailbox_keys.iter().any(|key| key == &poller.mailbox_key)
            }));
    let error_code = if snapshot.pollers.is_empty() || scope_mismatch {
        Some("no_pollers")
    } else if snapshot.pollers.iter().any(|poller| poller.paused) {
        Some("polling_paused")
    } else if snapshot.pollers.iter().any(|poller| poller.consecutive_failures > 0) {
        Some("polling_failures")
    } else {
        None
    };
    Ok(AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(super::timestamp(snapshot.observed_at)),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::timestamp(snapshot.observed_at),
            "pollers": snapshot.pollers.into_iter().map(|poller| json!({
                "mailbox_key": poller.mailbox_key,
                "paused": poller.paused,
                "consecutive_failures": poller.consecutive_failures,
                "last_poll_at": super::timestamp(poller.last_poll_at),
                "next_poll_at": super::timestamp(poller.next_poll_at),
                "state_version": poller.state_version,
                "updated_at": super::timestamp(poller.updated_at),
            })).collect::<Vec<_>>(),
        })),
    })
}

/// 读取 realtime lag 与未解决 failure 的安全 dashboard 摘要。
async fn load_realtime_section(
    port: &dyn RealtimeAdminPort,
) -> Result<AdminDashboardSectionResponse, ()> {
    let snapshot = port.dashboard_snapshot().await.map_err(|_| ())?;
    if !snapshot.validate() {
        return Err(());
    }

    let total_lag = snapshot
        .lags
        .iter()
        .try_fold(0_i64, |total, entry| total.checked_add(entry.lag))
        .ok_or(())?;
    let error_code = if snapshot.failure.count > 0 {
        Some("unresolved_failure")
    } else if total_lag > 0 {
        Some("consumer_lag")
    } else {
        None
    };
    Ok(AdminDashboardSectionResponse {
        status: if error_code.is_some() {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(super::timestamp(snapshot.observed_at)),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::timestamp(snapshot.observed_at),
            "lags": snapshot.lags.into_iter().map(|entry| json!({
                "stream_key": entry.stream_key,
                "consumer_key": entry.consumer_key,
                "offset": entry.offset,
                "latest_sequence": entry.latest_sequence,
                "lag": entry.lag,
                "last_advanced_at": entry.last_advanced_at.map(super::timestamp),
            })).collect::<Vec<_>>(),
            "failure": {
                "count": snapshot.failure.count,
                "oldest_at": snapshot.failure.oldest_at.map(super::timestamp),
                "latest_code": snapshot.failure.latest_code,
            },
        })),
    })
}

fn map_credentials(
    snapshot: AdminCredentialHealthSnapshot,
    process: &DashboardSection<ProcessSnapshot>,
    dashboard_worlds: &[AdminCredentialWorld],
) -> AdminDashboardSectionResponse {
    let active_count = snapshot.scopes.iter().map(|scope| scope.active_count).sum::<i64>();
    let invalid_count = snapshot.scopes.iter().map(|scope| scope.invalid_count).sum::<i64>();
    let restart_required = credential_restart_required(&snapshot, process);
    let availability_worlds = if dashboard_worlds.is_empty() {
        snapshot
            .scopes
            .iter()
            .filter(|scope| {
                scope.kind == AdminCredentialKind::MailCookie
                    && matches!(
                        scope.world,
                        AdminCredentialWorld::Main | AdminCredentialWorld::Isekai
                    )
            })
            .map(|scope| scope.world)
            .collect::<Vec<_>>()
    } else {
        dashboard_worlds.to_vec()
    };
    let mail_availability = availability_worlds
        .into_iter()
        .map(|world| mail_availability(&snapshot, world, restart_required))
        .collect::<Vec<_>>();
    let mail_ready = if dashboard_worlds.is_empty() {
        true
    } else {
        dashboard_worlds.iter().copied().all(|world| mail_world_ready(&snapshot, world))
    };
    let status =
        if !mail_ready || active_count == 0 || invalid_count > 0 || restart_required != Some(false)
        {
            DashboardStatus::Degraded
        } else {
            DashboardStatus::Ok
        };
    let error_code = if active_count == 0 || !mail_ready {
        Some("no_active_credentials".to_owned())
    } else if invalid_count > 0 {
        Some("invalid_credentials".to_owned())
    } else if restart_required == Some(true) {
        Some("credentials_restart_required".to_owned())
    } else if restart_required.is_none() {
        Some("credentials_restart_unknown".to_owned())
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: status.as_str().to_owned(),
        observed_at: Some(super::timestamp(snapshot.observed_at)),
        error_code,
        data: Some(json!({
            "active_count": active_count,
            "disabled_count": snapshot.scopes.iter().map(|scope| scope.disabled_count).sum::<i64>(),
            "invalid_count": invalid_count,
            "last_admin_change_at": snapshot.last_admin_change_at.map(super::timestamp),
            "restart_required": restart_required,
            "availability": mail_availability,
            "scopes": snapshot.scopes.into_iter().map(|scope| json!({
                "kind": scope.kind.as_str(),
                "world": scope.world.as_str(),
                "active_count": scope.active_count,
                "disabled_count": scope.disabled_count,
                "invalid_count": scope.invalid_count,
                "updated_at": scope.updated_at.map(super::timestamp),
                "restart_required": restart_required,
            })).collect::<Vec<_>>(),
        })),
    }
}

/// 生成旧版 keymanagement dashboard 使用的 MoogleMail world availability 投影。
fn mail_availability(
    snapshot: &AdminCredentialHealthSnapshot,
    world: AdminCredentialWorld,
    restart_required: Option<bool>,
) -> serde_json::Value {
    let scope = snapshot
        .scopes
        .iter()
        .find(|scope| scope.kind == AdminCredentialKind::MailCookie && scope.world == world);
    json!({
        "world": world.as_str(),
        "available": scope.map_or(0, |scope| scope.active_count),
        "invalid": scope.map_or(0, |scope| scope.invalid_count),
        "restart_required": restart_required,
        "updated_at": scope.and_then(|scope| scope.updated_at).map(super::timestamp),
    })
}

/// 旧版 dashboard 要求 main 与 isekai 两个 MoogleMail world 都可用。
fn mail_world_ready(snapshot: &AdminCredentialHealthSnapshot, world: AdminCredentialWorld) -> bool {
    snapshot.scopes.iter().any(|scope| {
        scope.kind == AdminCredentialKind::MailCookie
            && scope.world == world
            && scope.active_count > 0
            && scope.invalid_count == 0
            && scope.updated_at.is_some()
    })
}

/// 按旧版 servicehealth 语义，比较最近凭据变更和 Worker 启动时间。
///
/// 无凭据变更时可确定不需要重启；有变更但没有可验证的 Worker 心跳时返回未知，避免把
/// 未观测到的进程误报成已加载新凭据。
fn credential_restart_required(
    snapshot: &AdminCredentialHealthSnapshot,
    process: &DashboardSection<ProcessSnapshot>,
) -> Option<bool> {
    let Some(last_admin_change_at) = snapshot.last_admin_change_at else {
        return Some(false);
    };
    let process = process.data.as_ref()?;
    let mut worker_starts = process
        .processes
        .iter()
        .filter(|state| {
            state.service_name == ServiceName::Worker && state.process_role == ProcessRole::Worker
        })
        .map(|state| state.started_at);
    let first_worker_start = worker_starts.next()?;
    Some(
        last_admin_change_at >= first_worker_start
            || worker_starts.any(|started_at| last_admin_change_at >= started_at),
    )
}

fn map_worker(status: WorkerControlStatus) -> AdminDashboardWorkerResponse {
    if status.condition != WorkerControlCondition::Available {
        return AdminDashboardWorkerResponse {
            condition: status.condition.as_str().to_owned(),
            state: String::new(),
            ready: false,
            version: String::new(),
            mail_pollers: 0,
        };
    }
    AdminDashboardWorkerResponse {
        condition: status.condition.as_str().to_owned(),
        state: status.state,
        ready: status.ready,
        version: status.version,
        mail_pollers: status.mail_pollers,
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
    unknown_section_with_code("source_unavailable")
}

fn unknown_section_with_code(error_code: &'static str) -> AdminDashboardSectionResponse {
    AdminDashboardSectionResponse {
        status: DashboardStatus::Unknown.as_str().to_owned(),
        observed_at: None,
        error_code: Some(error_code.to_owned()),
        data: None,
    }
}

fn map_alerting(health: OperationsAlertHealth) -> AdminDashboardSectionResponse {
    let status = if !health.enabled {
        DashboardStatus::Unknown
    } else if health.failed_batch_count > 0
        || health.manual_reconcile_count > 0
        || health.claimed_batch_count > 0
        || health.due_signal_count > 0
    {
        DashboardStatus::Degraded
    } else {
        DashboardStatus::Ok
    };
    let error_code = if health.manual_reconcile_count > 0 {
        Some("manual_reconcile".to_owned())
    } else if health.failed_batch_count > 0 {
        Some("delivery_failed".to_owned())
    } else if !health.last_error_code.is_empty() {
        Some(health.last_error_code.clone())
    } else {
        None
    };
    AdminDashboardSectionResponse {
        status: status.as_str().to_owned(),
        observed_at: health.last_observed_at.map(super::timestamp),
        error_code,
        data: Some(json!({
            "enabled": health.enabled,
            "active_signal_count": health.active_signal_count,
            "due_signal_count": health.due_signal_count,
            "claimed_batch_count": health.claimed_batch_count,
            "succeeded_batch_count": health.succeeded_batch_count,
            "failed_batch_count": health.failed_batch_count,
            "manual_reconcile_count": health.manual_reconcile_count,
            "last_observed_at": health.last_observed_at.map(super::timestamp),
            "last_success_at": health.last_success_at.map(super::timestamp),
            "last_failure_at": health.last_failure_at.map(super::timestamp),
            "last_error_code": health.last_error_code,
        })),
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
