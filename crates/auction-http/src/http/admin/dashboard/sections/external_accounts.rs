//! Admin dashboard 外部账号分区。
//! 只合并 Worker 的进程内快照与既有安全投影：邮件轮询暂停来自 Admin 邮件 owner，凭据可用性
//! 来自既有凭据快照。控制面不可用时整区 unknown；子投影缺失时只在对应嵌套对象标 unavailable，
//! 不用空列表或零值冒充“没有暂停”或“凭据齐全”。

use auction_application::admin::{
    AdminCredentialHealthSnapshot, AdminCredentialKind, AdminCredentialWorld,
    AdminMailDashboardSnapshot,
};
use auction_application::external_accounts::ExternalAccountStatus;
use auction_application::operations::{WorkerControlCondition, WorkerControlExternalAccounts};
use auction_contracts::AdminDashboardSectionResponse;
use auction_observability::DashboardStatus;
use serde_json::{Value, json};

use super::super::super::timestamp;
use super::fallback::unknown_section_with_code;

pub(crate) fn map_external_accounts(
    accounts: WorkerControlExternalAccounts,
    mail: Option<&AdminMailDashboardSnapshot>,
    credentials: Option<&AdminCredentialHealthSnapshot>,
    dashboard_worlds: &[AdminCredentialWorld],
) -> AdminDashboardSectionResponse {
    let WorkerControlExternalAccounts { condition, status } = accounts;
    let status = match (condition, status) {
        (WorkerControlCondition::Available, Some(status)) if status.validate() => status,
        (WorkerControlCondition::Available, _) => {
            return unknown_section_with_code("source_incompatible");
        }
        _ => return unknown_section_with_code("source_unavailable"),
    };
    let observed_at = timestamp(status.observed_at);
    // 最近一次已结束维护失败即降级；另一个世界仍在运行不掩盖该失败。
    let latest_maintenance_failed = status.maintenance.last_failure_at.is_some_and(|failure| {
        status.maintenance.last_success_at.is_none_or(|success| failure > success)
    });
    AdminDashboardSectionResponse {
        status: if latest_maintenance_failed {
            DashboardStatus::Degraded.as_str().to_owned()
        } else {
            DashboardStatus::Ok.as_str().to_owned()
        },
        observed_at: Some(observed_at.clone()),
        error_code: None,
        data: Some(json!({
            "observed_at": observed_at,
            "mail": map_mail(&status, mail),
            "forum": {
                "last_request_at": optional_timestamp(status.forum.last_request_at),
                "last_success_at": optional_timestamp(status.forum.last_success_at),
                "last_failure_at": optional_timestamp(status.forum.last_failure_at),
                "write_unknown_count": status.forum.write_unknown_count,
            },
            "maintenance": {
                "state": status.maintenance.state.as_str(),
                "last_started_at": optional_timestamp(status.maintenance.last_started_at),
                "last_success_at": optional_timestamp(status.maintenance.last_success_at),
                "last_failure_at": optional_timestamp(status.maintenance.last_failure_at),
                "last_canceled_at": optional_timestamp(status.maintenance.last_canceled_at),
                "running_age_seconds": status.maintenance.running_age_seconds,
            },
            "credentials": map_credentials(credentials, dashboard_worlds),
        })),
    }
}

fn map_mail(status: &ExternalAccountStatus, mail: Option<&AdminMailDashboardSnapshot>) -> Value {
    json!({
        "busy": status.mail.busy,
        "busy_age_seconds": status.mail.busy_age_seconds,
        "gate_wait_count": status.mail.gate_wait_count,
        "gate_wait_seconds_total": status.mail.gate_wait_seconds_total,
        "request_count": status.mail.request_count,
        "last_request_at": optional_timestamp(status.mail.last_request_at),
        "last_success_at": optional_timestamp(status.mail.last_success_at),
        "last_failure_at": optional_timestamp(status.mail.last_failure_at),
        "pollers": map_pollers(mail),
    })
}

fn map_pollers(mail: Option<&AdminMailDashboardSnapshot>) -> Value {
    let Some(snapshot) = mail else {
        return json!({ "status": "unavailable", "items": [] });
    };
    json!({
        "status": "available",
        "items": snapshot.pollers.iter().map(|poller| json!({
            "mailbox_key": poller.mailbox_key.clone(),
            "paused": poller.paused,
            // 暂停期间轮询状态不再推进；该时间就是暂停命令写入后最后一次状态更新时间。
            "paused_since": poller.paused_since.map(timestamp),
            "consecutive_failures": poller.consecutive_failures,
            "last_poll_at": timestamp(poller.last_poll_at),
            "next_poll_at": timestamp(poller.next_poll_at),
        })).collect::<Vec<_>>(),
    })
}

fn map_credentials(
    credentials: Option<&AdminCredentialHealthSnapshot>,
    dashboard_worlds: &[AdminCredentialWorld],
) -> Value {
    let Some(snapshot) = credentials else {
        return json!({ "status": "unavailable", "items": [] });
    };
    json!({
        "status": "available",
        "items": snapshot.scopes.iter()
            .filter(|scope| matches!(
                scope.kind,
                AdminCredentialKind::MailCookie | AdminCredentialKind::ForumCookie
            ))
            .filter(|scope| {
                dashboard_worlds.is_empty()
                    || scope.world == AdminCredentialWorld::Shared
                    || dashboard_worlds.contains(&scope.world)
            })
            .map(|scope| json!({
                "world": scope.world.as_str(),
                "kind": scope.kind.as_str(),
                "available": scope.active_count,
                "invalid": scope.invalid_count,
            }))
            .collect::<Vec<_>>(),
    })
}

fn optional_timestamp(value: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    value.map(timestamp)
}

#[cfg(test)]
mod tests {
    use auction_application::admin::{
        AdminCredentialHealthScope, AdminCredentialHealthSnapshot, AdminCredentialKind,
        AdminCredentialWorld,
    };
    use auction_application::external_accounts::{
        ExternalAccountStatus, ExternalForumAccountStatus, ExternalMailAccountStatus,
        ExternalMaintenanceState, ExternalMaintenanceStatus,
    };
    use auction_application::operations::{WorkerControlCondition, WorkerControlExternalAccounts};
    use chrono::{TimeZone, Utc};

    use super::map_external_accounts;

    fn status(state: ExternalMaintenanceState) -> ExternalAccountStatus {
        let observed_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (last_success_at, last_failure_at, last_canceled_at, running_age_seconds) = match state
        {
            ExternalMaintenanceState::Idle => (None, None, None, None),
            ExternalMaintenanceState::Running => (None, None, None, Some(0)),
            ExternalMaintenanceState::Succeeded => (Some(observed_at), None, None, None),
            ExternalMaintenanceState::Failed => (None, Some(observed_at), None, None),
            ExternalMaintenanceState::Canceled => (None, None, Some(observed_at), None),
        };
        ExternalAccountStatus {
            observed_at,
            mail: ExternalMailAccountStatus {
                busy: false,
                busy_age_seconds: None,
                gate_wait_count: 0,
                gate_wait_seconds_total: 0,
                request_count: 0,
                last_request_at: None,
                last_success_at: None,
                last_failure_at: None,
            },
            forum: ExternalForumAccountStatus {
                last_request_at: None,
                last_success_at: None,
                last_failure_at: None,
                write_unknown_count: 0,
            },
            maintenance: ExternalMaintenanceStatus {
                state,
                last_started_at: Some(observed_at),
                last_success_at,
                last_failure_at,
                last_canceled_at,
                running_age_seconds,
            },
        }
    }

    #[test]
    fn unavailable_control_source_returns_unknown_without_data() {
        let section = map_external_accounts(
            WorkerControlExternalAccounts::unavailable(WorkerControlCondition::Timeout),
            None,
            None,
            &[],
        );
        assert_eq!(section.status, "unknown");
        assert!(section.data.is_none());
        assert_eq!(section.error_code.as_deref(), Some("source_unavailable"));
    }

    #[test]
    fn available_snapshot_reports_ok_and_nested_unavailable_projections() {
        let section = map_external_accounts(
            WorkerControlExternalAccounts {
                condition: WorkerControlCondition::Available,
                status: Some(status(ExternalMaintenanceState::Succeeded)),
            },
            None,
            None,
            &[],
        );
        assert_eq!(section.status, "ok");
        let data = section.data.expect("数据");
        assert_eq!(data["mail"]["pollers"]["status"], "unavailable");
        assert_eq!(data["mail"]["pollers"]["items"].as_array().map(Vec::len), Some(0));
        assert_eq!(data["credentials"]["status"], "unavailable");
        assert_eq!(data["maintenance"]["state"], "succeeded");
    }

    #[test]
    fn shared_credentials_survive_configured_world_filter() {
        let observed_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let credentials = AdminCredentialHealthSnapshot {
            observed_at,
            last_admin_change_at: None,
            scopes: vec![AdminCredentialHealthScope {
                kind: AdminCredentialKind::MailCookie,
                world: AdminCredentialWorld::Shared,
                active_count: 1,
                disabled_count: 0,
                invalid_count: 0,
                updated_at: Some(observed_at),
            }],
        };
        let section = map_external_accounts(
            WorkerControlExternalAccounts {
                condition: WorkerControlCondition::Available,
                status: Some(status(ExternalMaintenanceState::Succeeded)),
            },
            None,
            Some(&credentials),
            &[AdminCredentialWorld::Main],
        );
        let data = section.data.expect("数据");
        let items = data["credentials"]["items"].as_array().expect("凭据数组");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["world"], "shared");
        assert_eq!(items[0]["available"], 1);
    }

    #[test]
    fn failed_maintenance_degrades_section() {
        let section = map_external_accounts(
            WorkerControlExternalAccounts {
                condition: WorkerControlCondition::Available,
                status: Some(status(ExternalMaintenanceState::Failed)),
            },
            None,
            None,
            &[],
        );
        assert_eq!(section.status, "degraded");
    }
}
