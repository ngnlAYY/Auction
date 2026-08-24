use auction_application::{WorkerControlCondition, WorkerControlStatus};
use auction_contracts::{
    AdminDashboardHealthResponse, AdminDashboardResponse, AdminDashboardSectionResponse,
    AdminDashboardWorkerResponse,
};
use auction_observability::{
    AdminDashboardSnapshot, DashboardSection, HealthSummary, ProcessSnapshot, ProcessState,
    ServiceName, TracingHealth,
};
use serde_json::json;

use super::AdminDashboardSections;

pub(super) fn map_dashboard(
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
        observed_at: section.observed_at.map(super::super::timestamp),
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
        "observed_at": snapshot.observed_at.map(super::super::timestamp),
    })
}

fn map_process(process: ProcessState) -> serde_json::Value {
    json!({
        "service_name": process.service_name.as_str(),
        "process_role": process.process_role.as_str(),
        "instance_id": process.instance_id,
        "version": process.version,
        "started_at": super::super::timestamp(process.started_at),
        "status": process.status.as_str(),
        "readiness": process.readiness,
        "audit_failure_count": process.audit_failure_count,
        "tracing": map_tracing(process.tracing),
        "observed_at": super::super::timestamp(process.observed_at),
        "expires_at": super::super::timestamp(process.expires_at),
    })
}

fn map_tracing(health: TracingHealth) -> serde_json::Value {
    json!({
        "status": health.status.as_str(),
        "dropped_span_count": health.dropped_span_count,
        "export_failure_count": health.export_failure_count,
        "last_success_at": health.last_success_at.map(super::super::timestamp),
        "last_failure_at": health.last_failure_at.map(super::super::timestamp),
        "last_error_code": health.last_error_code,
    })
}

fn service_role_name(service: ServiceName) -> &'static str {
    match service {
        ServiceName::Public => "public",
        ServiceName::Admin => "admin",
        ServiceName::Worker => "worker",
    }
}
