//! Admin 告警台 HTTP 客户端。
//! 浏览器只接收告警台的安全投影；确认/静默命令携带稳定 `command_id`，同一输入重试复用
//! 同一 ID 与服务端摘要。页面不能把静默当作阈值修改，服务端也不会改变 signal 状态。

#[cfg(test)]
use std::rc::Rc;

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
#[cfg(test)]
use auction_web_shared::TransportError;
use gloo_net::http::Method;
use uuid::Uuid;

use super::AdminWriteFailure;
use super::transport::WriteRequest;
use super::{
    AdminApiFailure, invalid_request_error, invalid_response_error, response_error,
    response_failure, send_get, valid_count,
};

/// 读取一页告警台条目；`status` 为 active/resolved/all。
pub(crate) async fn fetch_admin_alerts(
    status: &str,
    page: i64,
    page_size: i64,
) -> Result<AdminAlertPage, String> {
    let url = alert_list_url(status, page, page_size)?;
    let body = read_alert_response(&url).await.map_err(|failure| failure.code)?;
    decode_alert_page(&body, status, page, page_size)
}

fn alert_list_url(status: &str, page: i64, page_size: i64) -> Result<String, String> {
    super::pagination::validate(page, page_size, 100, 100_000)?;
    if !matches!(status, "active" | "resolved" | "all") {
        return Err("invalid_request".to_owned());
    }
    Ok(format!(
        "/api/v1/admin/alerts?status={status}&page={page}&page_size={page_size}&include=incident_generation"
    ))
}

fn decode_alert_page(
    body: &str,
    status: &str,
    page: i64,
    page_size: i64,
) -> Result<AdminAlertPage, String> {
    let payload: ApiResponse<AdminAlertPage> =
        serde_json::from_str(body).map_err(invalid_response_error)?;
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
    super::pagination::valid_page_echo(
        page,
        page_size,
        data.page,
        data.page_size,
        data.total,
        data.alerts.len(),
    ) && valid_count(data.active_total)
        && valid_count(data.resolved_total)
        && expected_total == Some(data.total)
        && data.alerts.iter().all(|alert| {
            valid_alert(alert) && required_active.is_none_or(|active| alert.active == active)
        })
}

/// 读取最近的 webhook 投递安全投影；只读，不触发重试。
pub(crate) async fn fetch_admin_webhooks(limit: usize) -> Result<Vec<AdminAlertWebhook>, String> {
    if limit == 0 || limit > 100 {
        return Err("invalid_request".to_owned());
    }
    let url = format!("/api/v1/admin/alerts/webhooks?limit={limit}");
    let body = read_alert_response(&url).await.map_err(|failure| failure.code)?;
    decode_webhook_page(&body, limit)
}

fn decode_webhook_page(body: &str, limit: usize) -> Result<Vec<AdminAlertWebhook>, String> {
    let payload: ApiResponse<AdminAlertWebhookPage> =
        serde_json::from_str(body).map_err(invalid_response_error)?;
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

/// 导航待办计数（显式请求未结案结算退件数）；失败时导航不显示徽标而不是猜测数量。
pub(crate) async fn fetch_admin_pending_counts() -> Result<AdminPendingCounts, String> {
    let response = send_get("/api/v1/admin/pending-counts?include=settlement_returns").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminPendingCounts> =
        response.json().await.map_err(invalid_response_error)?;
    let counts = payload.data;
    if !valid_pending_counts(&counts) {
        return Err(invalid_response_error("pending counts"));
    }
    Ok(counts)
}

/// 请求了退件计数时它必须出现；全部计数是非负安全整数。
fn valid_pending_counts(counts: &AdminPendingCounts) -> bool {
    valid_count(counts.delivery_manual_reconcile)
        && valid_count(counts.recovery_confirmation)
        && valid_count(counts.active_alerts)
        && counts.settlement_returns.is_some_and(valid_count)
}

/// 确认一条活动告警。
pub(crate) async fn acknowledge_admin_alert(
    command_id: Uuid,
    alert_id: &str,
    incident_generation: i64,
    reason: &str,
) -> Result<AdminAlertCommandResult, AdminWriteFailure> {
    let command = acknowledge_request(command_id, incident_generation, reason);
    submit_alert_command(command_id, alert_id, "acknowledge", command).await
}

/// 静默一条活动告警到 `silence_seconds` 之后。
pub(crate) async fn silence_admin_alert(
    command_id: Uuid,
    alert_id: &str,
    incident_generation: i64,
    silence_seconds: i64,
    reason: &str,
) -> Result<AdminAlertCommandResult, AdminWriteFailure> {
    let command = silence_request(command_id, incident_generation, silence_seconds, reason)?;
    submit_alert_command(command_id, alert_id, "silence", command).await
}

/// 请求体只用首次提交冻结的参数，重试复用原命令 ID 与所处置的事件代数。
fn acknowledge_request(
    command_id: Uuid,
    incident_generation: i64,
    reason: &str,
) -> AdminAlertAcknowledgeRequest {
    AdminAlertAcknowledgeRequest {
        command_id: command_id.to_string(),
        incident_generation,
        reason: reason.to_owned(),
    }
}

/// 静默时长越界在发送前失败关闭，不交给服务端裁剪；其余同 [`acknowledge_request`]。
fn silence_request(
    command_id: Uuid,
    incident_generation: i64,
    silence_seconds: i64,
    reason: &str,
) -> Result<AdminAlertSilenceRequest, AdminWriteFailure> {
    if !(60..=7 * 24 * 3_600).contains(&silence_seconds) {
        return Err(AdminWriteFailure::not_sent("invalid_request"));
    }
    Ok(AdminAlertSilenceRequest {
        command_id: command_id.to_string(),
        incident_generation,
        reason: reason.to_owned(),
        silence_seconds,
    })
}

async fn submit_alert_command<T: serde::Serialize>(
    command_id: Uuid,
    alert_id: &str,
    action: &str,
    command: T,
) -> Result<AdminAlertCommandResult, AdminWriteFailure> {
    if alert_id.len() != 64 || !alert_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AdminWriteFailure::not_sent("invalid_request"));
    }
    let url = format!("/api/v1/admin/alerts/{alert_id}/{action}");
    let request_body = serde_json::to_string(&command)
        .map_err(|error| AdminWriteFailure::not_sent(invalid_request_error(error)))?;
    let body = send_alert_command(&url, &request_body).await?;
    decode_alert_receipt(&body, command_id, alert_id, action)
}

fn decode_alert_receipt(
    body: &str,
    command_id: Uuid,
    alert_id: &str,
    action: &str,
) -> Result<AdminAlertCommandResult, AdminWriteFailure> {
    let payload: ApiResponse<AdminAlertCommandResponse> =
        serde_json::from_str(body).map_err(AdminWriteFailure::invalid_response)?;
    let result = payload.data.alert;
    if result.id != alert_id
        || result.action != action
        || (action == "silence" && result.silenced_until.is_none())
        || (action == "acknowledge" && result.silenced_until.is_some())
        || result.id.is_empty()
        || command_id.is_nil()
    {
        return Err(AdminWriteFailure::invalid_response("alert command"));
    }
    Ok(result)
}

async fn read_alert_response(url: &str) -> Result<String, AdminApiFailure> {
    #[cfg(test)]
    if let Some(stub) = dioxus::prelude::try_consume_context::<AlertResponseStub>() {
        let (status, response) =
            (stub.0)(url, None).map_err(|error| AdminApiFailure::client(error.code()))?;
        return if (200..300).contains(&status) {
            Ok(response)
        } else {
            Err(AdminApiFailure::from_http_body(status, &response))
        };
    }

    let response = send_get(url).await.map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    response.text().await.map_err(|error| AdminApiFailure::client(invalid_response_error(error)))
}

async fn send_alert_command(url: &str, body: &str) -> Result<String, AdminWriteFailure> {
    let request = WriteRequest::general(url, Method::POST);
    #[cfg(test)]
    if let Some(stub) = dioxus::prelude::try_consume_context::<AlertResponseStub>() {
        let (status, response) = (stub.0)(url, Some(body))
            .map_err(|error| AdminWriteFailure::outcome_unknown(error.code()))?;
        return if (200..300).contains(&status) {
            Ok(response)
        } else {
            Err(request.decode_stub_failure(status, &response))
        };
    }
    request.send_body(body).await?.text().await.map_err(AdminWriteFailure::invalid_response)
}

#[cfg(test)]
type AlertRequestStub = dyn Fn(&str, Option<&str>) -> Result<(u16, String), TransportError>;

#[cfg(test)]
#[derive(Clone)]
struct AlertResponseStub(Rc<AlertRequestStub>);

#[cfg(test)]
impl AdminApiFailure {
    /// 告警页宿主测试记录已序列化的请求，并提供原始 HTTP 回执或传输超时。
    pub(crate) fn stub_alert_responses(
        send: impl Fn(&str, Option<&str>) -> Result<(u16, String), TransportError> + 'static,
    ) {
        dioxus::prelude::provide_context(AlertResponseStub(Rc::new(send)));
    }
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
        && alert.incident_generation.is_some_and(|generation| generation >= 0)
        && alert.count >= 0
        && alert.age_seconds >= 0
        && alert.active == alert.resolved_at.is_none()
        && !alert.first_seen_at.is_empty()
        && !alert.last_seen_at.is_empty()
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{
        AdminAlert, AdminAlertPage, AdminPendingCounts, acknowledge_request, alert_list_url,
        decode_alert_page, silence_request, valid_alert, valid_alert_page, valid_pending_counts,
    };
    use crate::api::MAX_JSON_SAFE_INTEGER;

    /// 页面显式请求退件计数：缺失、负数或越界都失败关闭，导航不显示徽标而不是猜测数量。
    #[test]
    fn pending_counts_require_the_requested_settlement_returns_field() {
        let counts = |settlement_returns| AdminPendingCounts {
            delivery_manual_reconcile: 1,
            recovery_confirmation: 0,
            active_alerts: 2,
            settlement_returns,
        };
        assert!(valid_pending_counts(&counts(Some(3))));
        assert!(!valid_pending_counts(&counts(None)), "请求了退件计数却缺失");
        assert!(!valid_pending_counts(&counts(Some(-1))));
        assert!(!valid_pending_counts(&counts(Some(MAX_JSON_SAFE_INTEGER + 1))));
        let negative = AdminPendingCounts { active_alerts: -1, ..counts(Some(0)) };
        assert!(!valid_pending_counts(&negative));
    }

    fn alert(source: &str) -> AdminAlert {
        AdminAlert {
            id: "0".repeat(64),
            kind: "disk_capacity_low".to_owned(),
            scope: "backup".to_owned(),
            source: source.to_owned(),
            severity: "urgent".to_owned(),
            active: true,
            incident_generation: Some(1),
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
    fn alert_list_always_requests_the_incident_generation() {
        assert_eq!(
            alert_list_url("all", 2, 50).expect("告警列表 URL"),
            "/api/v1/admin/alerts?status=all&page=2&page_size=50&include=incident_generation"
        );
        assert!(alert_list_url("active&include=unknown", 1, 50).is_err());
    }

    #[test]
    fn alert_page_requires_a_nonnegative_integer_incident_generation() {
        let wire = include_str!("../../../auction-contracts/tests/fixtures/admin/alerts-list.json");
        let decode = |body: &str| decode_alert_page(body, "active", 1, 50);
        let page = decode(wire).expect("接受后端告警列表");
        assert_eq!(page.alerts[0].incident_generation, Some(2));
        assert_eq!(page.alerts[1].incident_generation, Some(1));
        assert!(page.alerts[1].acknowledged_at.is_some());
        assert!(page.alerts[1].silenced_until.is_some());

        let mut invalid: Value = serde_json::from_str(wire).expect("告警列表样本");
        invalid["data"]["alerts"][0]
            .as_object_mut()
            .expect("告警条目")
            .remove("incident_generation");
        assert!(decode(&invalid.to_string()).is_err());
        for generation in [json!(-1), json!(1.5), Value::Null] {
            invalid["data"]["alerts"][0]["incident_generation"] = generation;
            assert!(decode(&invalid.to_string()).is_err());
        }
    }

    #[test]
    fn frozen_alert_commands_serialize_to_fixed_wire_bytes() {
        // 用固定载荷核对事件代数与其余冻结参数都进入原样重试的请求体。
        let command_id = Uuid::from_u128(0x0123_4567_89ab_4cde_8f01_2345_6789_abcd);
        let acknowledge =
            serde_json::to_string(&acknowledge_request(command_id, 1, "disk replaced"))
                .expect("json");
        assert_eq!(
            acknowledge,
            r#"{"command_id":"01234567-89ab-4cde-8f01-23456789abcd","incident_generation":1,"reason":"disk replaced"}"#
        );
        let silence = silence_request(command_id, 2, 86_400, "maintenance")
            .map(|request| serde_json::to_string(&request).expect("json"))
            .expect("合法静默时长");
        assert_eq!(
            silence,
            r#"{"command_id":"01234567-89ab-4cde-8f01-23456789abcd","incident_generation":2,"reason":"maintenance","silence_seconds":86400}"#
        );
    }

    #[test]
    fn silence_duration_outside_server_bounds_is_rejected_before_sending() {
        let command_id = Uuid::from_u128(1);
        assert!(silence_request(command_id, 1, 59, "").is_err());
        assert!(silence_request(command_id, 1, 7 * 24 * 3_600 + 1, "").is_err());
        assert!(silence_request(command_id, 1, 60, "").is_ok());
        assert!(silence_request(command_id, 1, 7 * 24 * 3_600, "").is_ok());
    }

    #[test]
    fn alert_and_pending_counts_fixtures_preserve_opt_in_projections() {
        use auction_contracts::ApiResponse;

        let baseline = include_str!(
            "../../../auction-contracts/tests/fixtures/admin/alerts-list-default.json"
        );
        let payload: ApiResponse<AdminAlertPage> =
            serde_json::from_str(baseline).expect("告警缺省投影");
        assert!(payload.data.alerts.iter().all(|alert| alert.incident_generation.is_none()));
        assert!(decode_alert_page(baseline, "active", 1, 50).is_err());

        for (body, count) in [
            (
                include_str!("../../../auction-contracts/tests/fixtures/admin/pending-counts.json"),
                None,
            ),
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/pending-counts-settlement-returns.json"
                ),
                Some(3),
            ),
        ] {
            let payload: ApiResponse<AdminPendingCounts> =
                serde_json::from_str(body).expect("待办计数");
            assert_eq!(payload.data.settlement_returns, count);
            assert_eq!(valid_pending_counts(&payload.data), count.is_some());
        }
    }

    #[test]
    fn webhook_and_alert_command_contract_fixtures() {
        use uuid::Uuid;

        use super::{decode_alert_receipt, decode_webhook_page};
        use crate::api::contract_fixtures::{admin_fixture, checked};

        let body = admin_fixture!("alert-webhooks");
        let entries =
            checked(body, "/data/entries/0", "attempt_count", |body| decode_webhook_page(body, 1));
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!((entry.status.as_str(), entry.attempt_count), ("pending", 0));
        assert_eq!(entry.scope, "auction");
        assert!(entry.completed_at.is_none());
        assert!(decode_webhook_page(body, 0).is_err());

        let alert_id = "1796d9b615cd5b873fa150780021903075f6a01c1a16cdfff4ac5976c86e7b1c";
        for (body, action, command_id, silenced_until) in [
            (
                admin_fixture!("alert-command"),
                "acknowledge",
                "01234567-89ab-4cde-8f01-23456789abcd",
                None,
            ),
            (
                admin_fixture!("alert-silence-command"),
                "silence",
                "01234567-89ab-4cde-8f01-23456789abce",
                Some("2025-11-15T10:46:40.110000Z"),
            ),
        ] {
            let command_id = Uuid::parse_str(command_id).unwrap();
            let receipt = checked(body, "/data/alert", "action", |body| {
                decode_alert_receipt(body, command_id, alert_id, action)
            });
            assert_eq!(receipt.id, alert_id);
            assert_eq!(receipt.action, action);
            assert_eq!(receipt.silenced_until.as_deref(), silenced_until);
            assert!(decode_alert_receipt(body, command_id, &"ab".repeat(32), action).is_err());
        }
    }
}
