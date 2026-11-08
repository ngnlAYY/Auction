//! Admin 告警台 HTTP 客户端（W5.8）。
//! 浏览器只接收告警台的安全投影；确认/静默命令携带稳定 `command_id`，同一输入重试复用
//! 同一 ID 与服务端摘要。页面不能把静默当作阈值修改，服务端也不会改变 signal 状态。

use auction_contracts::{
    AdminAlertAcknowledgeRequest, AdminAlertCommandResponse, AdminAlertSilenceRequest, ApiResponse,
};
pub(crate) use auction_contracts::{
    AdminAlertCommandResultResponse as AdminAlertCommandResult,
    AdminAlertEntryResponse as AdminAlert, AdminAlertListResponse as AdminAlertPage,
    AdminPendingCountsResponse as AdminPendingCounts,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    AdminApiFailure, fetch_csrf_token, invalid_request_error, invalid_response_error,
    response_error, response_failure, send_get, send_request,
};

/// 读取一页告警台条目；`status` 为 active/resolved/all。
pub(crate) async fn fetch_admin_alerts(
    status: &str,
    source: &str,
    page: i64,
    page_size: i64,
) -> Result<AdminAlertPage, String> {
    if !matches!(status, "active" | "resolved" | "all")
        || page < 1
        || !(1..=100).contains(&page_size)
    {
        return Err("invalid_request".to_owned());
    }
    let mut url = format!("/api/v1/admin/alerts?status={status}&page={page}&page_size={page_size}");
    if !source.is_empty() {
        url.push_str("&source=");
        url.push_str(source);
    }
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminAlertPage> =
        response.json().await.map_err(invalid_response_error)?;
    let page_data = payload.data;
    if page_data.page != page
        || page_data.page_size != page_size
        || page_data.total < 0
        || page_data.active_total < 0
        || page_data.resolved_total < 0
        || page_data.active_total + page_data.resolved_total != page_data.total
        || page_data.alerts.len() as i64 > page_size
        || page_data.alerts.iter().any(|alert| !valid_alert(alert))
    {
        return Err(invalid_response_error("alert page"));
    }
    Ok(page_data)
}

/// 导航待办计数；失败时导航不显示徽标而不是猜测数量。
pub(crate) async fn fetch_admin_pending_counts() -> Result<AdminPendingCounts, String> {
    let response = send_get("/api/v1/admin/pending-counts").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminPendingCounts> =
        response.json().await.map_err(invalid_response_error)?;
    let counts = payload.data;
    if counts.delivery_manual_reconcile < 0
        || counts.recovery_confirmation < 0
        || counts.active_alerts < 0
    {
        return Err(invalid_response_error("pending counts"));
    }
    Ok(counts)
}

/// 确认一条活动告警。
pub(crate) async fn acknowledge_admin_alert(
    command_id: Uuid,
    alert_id: &str,
    reason: &str,
) -> Result<AdminAlertCommandResult, AdminApiFailure> {
    let command = AdminAlertAcknowledgeRequest {
        command_id: command_id.to_string(),
        reason: reason.to_owned(),
    };
    submit_alert_command(command_id, alert_id, "acknowledge", command).await
}

/// 静默一条活动告警到 `silence_seconds` 之后。
pub(crate) async fn silence_admin_alert(
    command_id: Uuid,
    alert_id: &str,
    silence_seconds: i64,
    reason: &str,
) -> Result<AdminAlertCommandResult, AdminApiFailure> {
    if !(60..=7 * 24 * 3_600).contains(&silence_seconds) {
        return Err(AdminApiFailure::client("invalid_request"));
    }
    let command = AdminAlertSilenceRequest {
        command_id: command_id.to_string(),
        reason: reason.to_owned(),
        silence_seconds,
    };
    submit_alert_command(command_id, alert_id, "silence", command).await
}

async fn submit_alert_command<T: serde::Serialize>(
    command_id: Uuid,
    alert_id: &str,
    action: &str,
    command: T,
) -> Result<AdminAlertCommandResult, AdminApiFailure> {
    if alert_id.len() != 64 || !alert_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AdminApiFailure::client("invalid_request"));
    }
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let url = format!("/api/v1/admin/alerts/{alert_id}/{action}");
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<AdminAlertCommandResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let result = payload.data.alert;
    if result.id != alert_id
        || result.action != action
        || (action == "silence" && result.silenced_until.is_none())
        || (action == "acknowledge" && result.silenced_until.is_some())
        || result.id.is_empty()
        || command_id.is_nil()
    {
        return Err(AdminApiFailure::client(invalid_response_error("alert command")));
    }
    Ok(result)
}

fn valid_alert(alert: &AdminAlert) -> bool {
    alert.id.len() == 64
        && alert.id.bytes().all(|byte| byte.is_ascii_hexdigit())
        && matches!(alert.severity.as_str(), "urgent" | "digest")
        && matches!(
            alert.source.as_str(),
            "process"
                | "delivery"
                | "fulfillment"
                | "recovery"
                | "mail"
                | "credential"
                | "lifecycle"
                | "forum"
                | "notification"
                | "backup"
                | "wal"
                | "reconciliation"
        )
        && !alert.kind.is_empty()
        && !alert.scope.is_empty()
        && !alert.runbook_anchor.is_empty()
        && alert.count >= 0
        && alert.age_seconds >= 0
        && alert.active == alert.resolved_at.is_none()
        && !alert.first_seen_at.is_empty()
        && !alert.last_seen_at.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{AdminAlert, valid_alert};

    fn alert(source: &str) -> AdminAlert {
        AdminAlert {
            id: "0".repeat(64),
            kind: "disk_capacity_low".to_owned(),
            scope: "backup".to_owned(),
            source: source.to_owned(),
            severity: "urgent".to_owned(),
            active: true,
            count: 1,
            age_seconds: 0,
            first_seen_at: "2026-11-07T00:00:00Z".to_owned(),
            last_seen_at: "2026-11-07T00:00:00Z".to_owned(),
            resolved_at: None,
            last_notified_at: None,
            next_notify_at: None,
            acknowledged_at: None,
            acknowledged_by: None,
            silenced_until: None,
            runbook_anchor: "alert-runbook-backup".to_owned(),
        }
    }

    #[test]
    fn host_side_alert_sources_are_accepted_by_the_console() {
        for source in ["backup", "wal", "reconciliation"] {
            assert!(valid_alert(&alert(source)), "source {source} must be displayed");
        }
        assert!(!valid_alert(&alert("unknown_source")));
    }
}
