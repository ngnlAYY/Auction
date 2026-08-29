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
    let observed_at = timestamp(snapshot.observed_at);
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
    AdminDashboardSectionResponse {
        status: status.as_str().to_owned(),
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "availability": mail_availability,
            "observed_at": observed_at,
            "last_admin_change_at": snapshot.last_admin_change_at.map(timestamp),
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
