use auction_application::{
    AdminCredentialHealthSnapshot, AdminCredentialKind, AdminCredentialWorld, AdminMailPort,
    OperationsAlertDeliverySnapshot, OperationsAlertFulfillmentSnapshot, OperationsAlertHealth,
    OperationsAlertRecoverySnapshot, RealtimeAdminPort,
};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::{
    DashboardSection, DashboardStatus, ProcessRole, ProcessSnapshot, ServiceName,
};
use serde_json::json;

pub(super) async fn load_mail_section(
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
        observed_at: Some(super::super::timestamp(snapshot.observed_at)),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::super::timestamp(snapshot.observed_at),
            "pollers": snapshot.pollers.into_iter().map(|poller| json!({
                "mailbox_key": poller.mailbox_key,
                "paused": poller.paused,
                "consecutive_failures": poller.consecutive_failures,
                "last_poll_at": super::super::timestamp(poller.last_poll_at),
                "next_poll_at": super::super::timestamp(poller.next_poll_at),
                "state_version": poller.state_version,
                "updated_at": super::super::timestamp(poller.updated_at),
            })).collect::<Vec<_>>(),
        })),
    })
}

pub(super) async fn load_realtime_section(
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
        observed_at: Some(super::super::timestamp(snapshot.observed_at)),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::super::timestamp(snapshot.observed_at),
            "lags": snapshot.lags.into_iter().map(|entry| json!({
                "stream_key": entry.stream_key,
                "consumer_key": entry.consumer_key,
                "offset": entry.offset,
                "latest_sequence": entry.latest_sequence,
                "lag": entry.lag,
                "last_advanced_at": entry.last_advanced_at.map(super::super::timestamp),
            })).collect::<Vec<_>>(),
            "failure": {
                "count": snapshot.failure.count,
                "oldest_at": snapshot.failure.oldest_at.map(super::super::timestamp),
                "latest_code": snapshot.failure.latest_code,
            },
        })),
    })
}

pub(super) fn map_fulfillment(
    snapshot: OperationsAlertFulfillmentSnapshot,
) -> AdminDashboardSectionResponse {
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
        observed_at: Some(super::super::timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::super::timestamp(chrono::Utc::now()),
            "pending": snapshot.pending,
            "blocked": snapshot.blocked,
            "failed": snapshot.failed,
            "manual_reconcile": snapshot.manual_reconcile,
            "oldest_backlog_age_seconds": snapshot.oldest_backlog_age_seconds,
        })),
    }
}

pub(super) fn map_delivery(
    snapshot: OperationsAlertDeliverySnapshot,
) -> AdminDashboardSectionResponse {
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
        observed_at: Some(super::super::timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::super::timestamp(chrono::Utc::now()),
            "water": {
                "pending": snapshot.pending,
                "failed": snapshot.failed,
                "manual_reconcile": snapshot.manual_reconcile,
                "oldest_pending_age_seconds": snapshot.oldest_pending_age_seconds,
                "oldest_manual_reconcile_age_seconds": snapshot.oldest_manual_reconcile_age_seconds,
                "last_update_at": snapshot.last_update_at.map(super::super::timestamp),
            },
        })),
    }
}

pub(super) fn map_recovery(
    snapshot: OperationsAlertRecoverySnapshot,
) -> AdminDashboardSectionResponse {
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
        observed_at: Some(super::super::timestamp(chrono::Utc::now())),
        error_code: error_code.map(ToOwned::to_owned),
        data: Some(json!({
            "observed_at": super::super::timestamp(chrono::Utc::now()),
            "worlds": snapshot.worlds.into_iter().map(|world| json!({
                "world": world.world,
                "balance_credits": world.balance_credits,
                "balance_status": world.balance_status,
                "evidence_status": world.evidence_status,
                "evidence_age_seconds": world.evidence_age_seconds,
                "balance_observed_at": world.balance_observed_at.map(super::super::timestamp),
                "evidence_last_success_at": world.evidence_last_success_at.map(super::super::timestamp),
                "updated_at": super::super::timestamp(world.updated_at),
            })).collect::<Vec<_>>(),
            "payment_overdue_count": snapshot.payment_overdue_count,
            "relist_pending_count": snapshot.relist_pending_count,
            "failed_action_count": snapshot.failed_action_count,
            "manual_reconcile_count": snapshot.manual_reconcile_count,
            "oldest_overdue_age_seconds": snapshot.oldest_overdue_age_seconds,
        })),
    }
}

pub(super) fn map_credentials(
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
        observed_at: Some(super::super::timestamp(snapshot.observed_at)),
        error_code,
        data: Some(json!({
            "active_count": active_count,
            "disabled_count": snapshot.scopes.iter().map(|scope| scope.disabled_count).sum::<i64>(),
            "invalid_count": invalid_count,
            "last_admin_change_at": snapshot.last_admin_change_at.map(super::super::timestamp),
            "restart_required": restart_required,
            "availability": mail_availability,
            "scopes": snapshot.scopes.into_iter().map(|scope| json!({
                "kind": scope.kind.as_str(),
                "world": scope.world.as_str(),
                "active_count": scope.active_count,
                "disabled_count": scope.disabled_count,
                "invalid_count": scope.invalid_count,
                "updated_at": scope.updated_at.map(super::super::timestamp),
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
        "updated_at": scope.and_then(|scope| scope.updated_at).map(super::super::timestamp),
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

pub(super) fn unknown_section() -> AdminDashboardSectionResponse {
    unknown_section_with_code("source_unavailable")
}

pub(super) fn unknown_section_with_code(error_code: &'static str) -> AdminDashboardSectionResponse {
    AdminDashboardSectionResponse {
        status: DashboardStatus::Unknown.as_str().to_owned(),
        observed_at: None,
        error_code: Some(error_code.to_owned()),
        data: None,
    }
}

pub(super) fn map_alerting(health: OperationsAlertHealth) -> AdminDashboardSectionResponse {
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
        observed_at: health.last_observed_at.map(super::super::timestamp),
        error_code,
        data: Some(json!({
            "enabled": health.enabled,
            "active_signal_count": health.active_signal_count,
            "due_signal_count": health.due_signal_count,
            "claimed_batch_count": health.claimed_batch_count,
            "succeeded_batch_count": health.succeeded_batch_count,
            "failed_batch_count": health.failed_batch_count,
            "manual_reconcile_count": health.manual_reconcile_count,
            "last_observed_at": health.last_observed_at.map(super::super::timestamp),
            "last_success_at": health.last_success_at.map(super::super::timestamp),
            "last_failure_at": health.last_failure_at.map(super::super::timestamp),
            "last_error_code": health.last_error_code,
        })),
    }
}
