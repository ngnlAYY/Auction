//! Admin dashboard 凭据 scope 聚合和 Worker 重启判断。

use auction_application::{
    AdminCredentialHealthSnapshot, AdminCredentialKind, AdminCredentialWorld,
};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::{
    DashboardSection, DashboardStatus, ProcessRole, ProcessSnapshot, ServiceName,
};
use chrono::{DateTime, Utc};
use serde_json::json;

use super::super::super::timestamp;

pub(crate) fn map_credentials(
    snapshot: AdminCredentialHealthSnapshot,
    process: &DashboardSection<ProcessSnapshot>,
    dashboard_worlds: &[AdminCredentialWorld],
) -> AdminDashboardSectionResponse {
    let observed_at = timestamp(snapshot.observed_at);
    let has_active = snapshot.scopes.iter().any(|scope| scope.active_count > 0);
    let has_invalid = snapshot.scopes.iter().any(|scope| scope.invalid_count > 0);
    let restart_required = credential_restart_required(&snapshot, process);
    let default_worlds = [AdminCredentialWorld::Main, AdminCredentialWorld::Isekai];
    let availability_worlds =
        if dashboard_worlds.is_empty() { &default_worlds[..] } else { dashboard_worlds };
    let mail_availability = availability_worlds
        .iter()
        .copied()
        .filter(|world| {
            !dashboard_worlds.is_empty()
                || snapshot.scopes.iter().any(|scope| {
                    scope.kind == AdminCredentialKind::MailCookie
                        && (scope.world == *world || scope.world == AdminCredentialWorld::Shared)
                })
        })
        .map(|world| mail_availability(&snapshot, world, restart_required))
        .collect::<Vec<_>>();
    let mail_ready = if dashboard_worlds.is_empty() {
        true
    } else {
        dashboard_worlds.iter().copied().all(|world| mail_world_ready(&snapshot, world))
    };
    let status = if !mail_ready || !has_active || has_invalid || restart_required != Some(false) {
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
    let availability = effective_mail_availability(snapshot, world);
    json!({
        "world": world.as_str(),
        "available": availability.active_count,
        "invalid": availability.invalid_count,
        "restart_required": restart_required,
        "updated_at": availability.updated_at.map(timestamp),
    })
}

fn mail_world_ready(snapshot: &AdminCredentialHealthSnapshot, world: AdminCredentialWorld) -> bool {
    let availability = effective_mail_availability(snapshot, world);
    availability.active_count > 0
        && availability.invalid_count == 0
        && availability.updated_at.is_some()
}

#[derive(Default)]
struct EffectiveMailAvailability {
    active_count: i64,
    invalid_count: i64,
    updated_at: Option<DateTime<Utc>>,
}

fn effective_mail_availability(
    snapshot: &AdminCredentialHealthSnapshot,
    world: AdminCredentialWorld,
) -> EffectiveMailAvailability {
    snapshot
        .scopes
        .iter()
        .filter(|scope| {
            scope.kind == AdminCredentialKind::MailCookie
                && (scope.world == world || scope.world == AdminCredentialWorld::Shared)
        })
        .fold(EffectiveMailAvailability::default(), |mut availability, scope| {
            availability.active_count =
                availability.active_count.saturating_add(scope.active_count);
            availability.invalid_count =
                availability.invalid_count.saturating_add(scope.invalid_count);
            availability.updated_at = availability.updated_at.max(scope.updated_at);
            availability
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
