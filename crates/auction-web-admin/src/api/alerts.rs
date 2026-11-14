//! Admin 告警台 HTTP 客户端。
//! 浏览器只接收告警台的安全投影；确认/静默命令携带稳定 `command_id`，同一输入重试复用
//! 同一 ID 与服务端摘要。页面不能把静默当作阈值修改，服务端也不会改变 signal 状态。

use auction_contracts::{
    AdminAlertAcknowledgeRequest, AdminAlertCommandResponse, AdminAlertSilenceRequest, ApiResponse,
};
pub(crate) use auction_contracts::{
    AdminAlertCommandResultResponse as AdminAlertCommandResult,
    AdminAlertEntryResponse as AdminAlert, AdminAlertListResponse as AdminAlertPage,
    AdminAlertWebhookEntryResponse as AdminAlertWebhook,
    AdminAlertWebhookListResponse as AdminAlertWebhookPage,
    AdminPendingCountsResponse as AdminPendingCounts,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    AdminApiFailure, MAX_JSON_SAFE_INTEGER, fetch_csrf_token, invalid_request_error,
    invalid_response_error, response_error, response_failure, send_get, send_request,
};

/// 读取一页告警台条目；`status` 为 active/resolved/all。
pub(crate) async fn fetch_admin_alerts(
    status: &str,
    page: i64,
    page_size: i64,
) -> Result<AdminAlertPage, String> {
    if !matches!(status, "active" | "resolved" | "all")
        || page < 1
        || !(1..=100).contains(&page_size)
    {
        return Err("invalid_request".to_owned());
    }
    let url = format!("/api/v1/admin/alerts?status={status}&page={page}&page_size={page_size}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminAlertPage> =
        response.json().await.map_err(invalid_response_error)?;
    let page_data = payload.data;
    if !valid_alert_page(status, page, page_size, &page_data) {
        return Err(invalid_response_error("alert page"));
    }
    Ok(page_data)
}

/// 告警台分页响应的失败关闭校验。`total` 是所选筛选下的条数：active 取 `active_total`，
/// resolved 取 `resolved_total`，all 取两者之和；两个分类计数不随筛选变化，只用于页头提示。
/// 条目也必须落在所选筛选内，已恢复的告警不能混进活动视图（反之亦然）。
fn valid_alert_page(status: &str, page: i64, page_size: i64, data: &AdminAlertPage) -> bool {
    let (expected_total, required_active) = match status {
        "active" => (Some(data.active_total), Some(true)),
        "resolved" => (Some(data.resolved_total), Some(false)),
        // 两类之和溢出说明计数已损坏，按不匹配处理。
        "all" => (data.active_total.checked_add(data.resolved_total), None),
        _ => return false,
    };
    data.page == page
        && data.page_size == page_size
        && crate::api::pagination::valid_response(data.page, data.page_size, data.total)
        && (0..=MAX_JSON_SAFE_INTEGER).contains(&data.active_total)
        && (0..=MAX_JSON_SAFE_INTEGER).contains(&data.resolved_total)
        && expected_total == Some(data.total)
        && i64::try_from(data.alerts.len()).is_ok_and(|count| count <= page_size.min(data.total))
        && data.alerts.iter().all(|alert| {
            valid_alert(alert) && required_active.is_none_or(|active| alert.active == active)
        })
}

/// 读取最近的 webhook 投递安全投影；只读，不触发重试。
pub(crate) async fn fetch_admin_webhooks(limit: usize) -> Result<Vec<AdminAlertWebhook>, String> {
    if limit == 0 || limit > 100 {
        return Err("invalid_request".to_owned());
    }
    let response = send_get(&format!("/api/v1/admin/alerts/webhooks?limit={limit}")).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminAlertWebhookPage> =
        response.json().await.map_err(invalid_response_error)?;
    let page = payload.data;
    if page.entries.len() > limit
        || page.entries.iter().any(|entry| {
            !matches!(
                entry.status.as_str(),
                "pending" | "sending" | "succeeded" | "failed" | "manual_reconcile"
            ) || entry.id.is_empty()
                || entry.kind.is_empty()
                || entry.scope.is_empty()
                || entry.source.is_empty()
        })
    {
        return Err(invalid_response_error("webhook page"));
    }
    Ok(page.entries)
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
    let command = acknowledge_request(command_id, reason);
    submit_alert_command(command_id, alert_id, "acknowledge", command).await
}

/// 静默一条活动告警到 `silence_seconds` 之后。
pub(crate) async fn silence_admin_alert(
    command_id: Uuid,
    alert_id: &str,
    silence_seconds: i64,
    reason: &str,
) -> Result<AdminAlertCommandResult, AdminApiFailure> {
    let command = silence_request(command_id, silence_seconds, reason)?;
    submit_alert_command(command_id, alert_id, "silence", command).await
}

/// 请求体只由冻结的命令 ID 与备注决定：同一输入重试得到逐字节相同的请求，服务端按原 ID 重放。
fn acknowledge_request(command_id: Uuid, reason: &str) -> AdminAlertAcknowledgeRequest {
    AdminAlertAcknowledgeRequest { command_id: command_id.to_string(), reason: reason.to_owned() }
}

/// 静默时长越界在发送前失败关闭，不交给服务端裁剪；其余同 [`acknowledge_request`]。
fn silence_request(
    command_id: Uuid,
    silence_seconds: i64,
    reason: &str,
) -> Result<AdminAlertSilenceRequest, AdminApiFailure> {
    if !(60..=7 * 24 * 3_600).contains(&silence_seconds) {
        return Err(AdminApiFailure::client("invalid_request"));
    }
    Ok(AdminAlertSilenceRequest {
        command_id: command_id.to_string(),
        reason: reason.to_owned(),
        silence_seconds,
    })
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
    use uuid::Uuid;

    use super::{
        AdminAlert, AdminAlertPage, MAX_JSON_SAFE_INTEGER, acknowledge_request, silence_request,
        valid_alert, valid_alert_page,
    };

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

    fn resolved_alert(source: &str) -> AdminAlert {
        AdminAlert {
            active: false,
            resolved_at: Some("2026-11-08T00:00:00Z".to_owned()),
            ..alert(source)
        }
    }

    /// 第 1 页、每页 50 条；分类计数显式给出，`total` 由用例按筛选语义填写。
    fn alert_page(
        alerts: Vec<AdminAlert>,
        total: i64,
        active_total: i64,
        resolved_total: i64,
    ) -> AdminAlertPage {
        AdminAlertPage { alerts, page: 1, page_size: 50, total, active_total, resolved_total }
    }

    #[test]
    fn filtered_alert_page_total_counts_only_the_selected_category() {
        // 两类都非空：active/resolved 视图的 total 只等于所选分类的计数，all 才是两者之和。
        let active = alert_page(vec![alert("backup")], 1, 1, 3);
        assert!(valid_alert_page("active", 1, 50, &active));
        let resolved = alert_page(vec![resolved_alert("backup")], 3, 1, 3);
        assert!(valid_alert_page("resolved", 1, 50, &resolved));
        let all = alert_page(vec![alert("backup"), resolved_alert("wal")], 4, 1, 3);
        assert!(valid_alert_page("all", 1, 50, &all));

        // total 与所选分类计数不符一律拒绝，包括把两类之和当作筛选视图的总数。
        let summed_active = alert_page(vec![alert("backup")], 4, 1, 3);
        assert!(!valid_alert_page("active", 1, 50, &summed_active));
        let summed_resolved = alert_page(vec![resolved_alert("backup")], 4, 1, 3);
        assert!(!valid_alert_page("resolved", 1, 50, &summed_resolved));
        let partial_all = alert_page(vec![alert("backup")], 3, 1, 3);
        assert!(!valid_alert_page("all", 1, 50, &partial_all));
    }

    #[test]
    fn alert_page_entries_must_match_the_selected_filter() {
        let mixed = vec![alert("backup"), resolved_alert("wal")];
        assert!(!valid_alert_page("active", 1, 50, &alert_page(mixed.clone(), 2, 2, 1)));
        assert!(!valid_alert_page("resolved", 1, 50, &alert_page(mixed.clone(), 2, 1, 2)));
        assert!(valid_alert_page("all", 1, 50, &alert_page(mixed, 3, 2, 1)));
    }

    #[test]
    fn alert_page_counts_and_bounds_fail_closed() {
        let page = alert_page(vec![alert("backup")], 1, 1, 0);
        assert!(valid_alert_page("active", 1, 50, &page));
        // 回显的分页参数必须与请求一致，筛选必须属于闭集。
        assert!(!valid_alert_page("active", 2, 50, &page));
        assert!(!valid_alert_page("active", 1, 20, &page));
        assert!(!valid_alert_page("silenced", 1, 50, &page));
        // 条数不能超过 min(page_size, total)。
        assert!(!valid_alert_page("active", 1, 50, &alert_page(vec![alert("backup")], 0, 0, 0)));
        let overfull = AdminAlertPage {
            page_size: 1,
            ..alert_page(vec![alert("backup"), alert("wal")], 2, 2, 0)
        };
        assert!(!valid_alert_page("active", 1, 1, &overfull));
        // 计数为负、越过 JSON 安全整数或相加溢出都视为损坏。
        assert!(!valid_alert_page("active", 1, 50, &alert_page(Vec::new(), 0, 0, -1)));
        let unsafe_count = MAX_JSON_SAFE_INTEGER + 1;
        let unsafe_resolved = alert_page(Vec::new(), 0, 0, unsafe_count);
        assert!(!valid_alert_page("active", 1, 50, &unsafe_resolved));
        let overflow = alert_page(Vec::new(), i64::MAX.wrapping_add(1), i64::MAX, 1);
        assert!(!valid_alert_page("all", 1, 50, &overflow));
    }

    #[test]
    fn host_side_alert_sources_are_accepted_by_the_console() {
        for source in ["backup", "wal", "reconciliation"] {
            assert!(valid_alert(&alert(source)), "source {source} must be displayed");
        }
        assert!(!valid_alert(&alert("unknown_source")));
    }

    #[test]
    fn frozen_alert_commands_serialize_byte_for_byte() {
        let command_id = Uuid::from_u128(0x0123_4567_89ab_4cde_8f01_2345_6789_abcd);
        let acknowledge = || serde_json::to_vec(&acknowledge_request(command_id, "disk replaced"));
        assert_eq!(acknowledge().expect("json"), acknowledge().expect("json"));
        let silence = || {
            silence_request(command_id, 86_400, "maintenance")
                .map(|request| serde_json::to_vec(&request).expect("json"))
        };
        assert_eq!(silence(), silence());
        assert!(silence().is_ok());
    }

    #[test]
    fn silence_duration_outside_server_bounds_is_rejected_before_sending() {
        let command_id = Uuid::from_u128(1);
        assert!(silence_request(command_id, 59, "").is_err());
        assert!(silence_request(command_id, 7 * 24 * 3_600 + 1, "").is_err());
        assert!(silence_request(command_id, 60, "").is_ok());
        assert!(silence_request(command_id, 7 * 24 * 3_600, "").is_ok());
    }
}
