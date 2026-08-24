//! Admin dashboard 凭据 scope 聚合和 Worker 重启判断。

use auction_application::{
    AdminCredentialHealthSnapshot, AdminCredentialKind, AdminCredentialWorld,
};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::{
    DashboardSection, DashboardStatus, ProcessRole, ProcessSnapshot, ServiceName,
};
use serde_json::json;

use super::super::super::timestamp;

pub(crate) fn map_credentials(
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
        observed_at: Some(timestamp(snapshot.observed_at)),
        error_code,
        data: Some(json!({
            "active_count": active_count,
            "disabled_count": snapshot.scopes.iter().map(|scope| scope.disabled_count).sum::<i64>(),
            "invalid_count": invalid_count,
            "last_admin_change_at": snapshot.last_admin_change_at.map(timestamp),
            "restart_required": restart_required,
            "availability": mail_availability,
            "scopes": snapshot.scopes.into_iter().map(|scope| json!({
                "kind": scope.kind.as_str(),
                "world": scope.world.as_str(),
                "active_count": scope.active_count,
                "disabled_count": scope.disabled_count,
                "invalid_count": scope.invalid_count,
                "updated_at": scope.updated_at.map(timestamp),
                "restart_required": restart_required,
            })).collect::<Vec<_>>(),
        })),
    }
}

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
        "updated_at": scope.and_then(|scope| scope.updated_at).map(timestamp),
    })
}

fn mail_world_ready(snapshot: &AdminCredentialHealthSnapshot, world: AdminCredentialWorld) -> bool {
    snapshot.scopes.iter().any(|scope| {
        scope.kind == AdminCredentialKind::MailCookie
            && scope.world == world
            && scope.active_count > 0
            && scope.invalid_count == 0
            && scope.updated_at.is_some()
    })
}

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
