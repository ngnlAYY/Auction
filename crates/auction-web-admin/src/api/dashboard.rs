//! Admin dashboard 聚合投影客户端。
//! 页面只读取各 owner 提供的安全摘要；数据库行、delivery payload、Cookie 和外部错误正文
//! 不会进入浏览器。DTO 与后端 dashboard 响应保持字段闭集，便于旧版运维信息逐项迁移。

pub(crate) use auction_contracts::{
    AdminDashboardHealthResponse as AdminDashboardHealth,
    AdminDashboardWorkerResponse as AdminDashboardWorker,
};
use auction_contracts::{AdminDashboardResponse, AdminDashboardSectionResponse};
use auction_web_shared::wire::{json_exact_keys as exact_keys, json_exact_object as exact_object};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use super::mail::AdminMailPolling;
use super::{ApiResponse, invalid_response_error, response_error, send_get};

/// Dashboard section 的浏览器类型化投影；wire 层仍保留 owner 构造的有界 JSON。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminDashboardSection<T> {
    pub status: String,
    pub observed_at: Option<String>,
    pub error_code: Option<String>,
    pub data: Option<T>,
}

/// 进程 tracing 安全摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminTracingData {
    pub status: String,
    pub dropped_span_count: i64,
    pub export_failure_count: i64,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub last_error_code: String,
}

/// 单个进程的健康安全投影。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminProcessData {
    pub service_name: String,
    pub process_role: String,
    pub instance_id: String,
    pub version: String,
    pub started_at: String,
    pub status: String,
    pub readiness: bool,
    pub audit_failure_count: i64,
    pub tracing: AdminTracingData,
    pub observed_at: String,
    pub expires_at: String,
}

/// 进程 dashboard section 的数据。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminProcessSectionData {
    pub processes: Vec<AdminProcessData>,
    pub summary: AdminDashboardHealth,
    pub observed_at: String,
}

/// Realtime dashboard 的 lag 条目。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRealtimeLag {
    pub stream_key: String,
    pub consumer_key: String,
    pub offset: i64,
    pub latest_sequence: i64,
    pub lag: i64,
    pub last_advanced_at: Option<String>,
}

/// Realtime 未解决失败的安全摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRealtimeFailure {
    pub count: i64,
    pub oldest_at: Option<String>,
    pub latest_code: String,
}

/// Realtime dashboard section 的数据。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRealtimeData {
    pub total_lag: i64,
    pub lags: Vec<AdminRealtimeLag>,
    pub failure: AdminRealtimeFailure,
    pub observed_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminAlertingData {
    pub enabled: bool,
    pub active_signal_count: i64,
    pub due_signal_count: i64,
    pub claimed_batch_count: i64,
    pub succeeded_batch_count: i64,
    pub failed_batch_count: i64,
    pub manual_reconcile_count: i64,
    #[serde(default)]
    pub webhook_pending_count: i64,
    #[serde(default)]
    pub webhook_sending_count: i64,
    #[serde(default)]
    pub webhook_failed_count: i64,
    #[serde(default)]
    pub webhook_manual_reconcile_count: i64,
    #[serde(default)]
    pub last_observed_at: Option<String>,
    #[serde(default)]
    pub last_success_at: Option<String>,
    #[serde(default)]
    pub last_failure_at: Option<String>,
    #[serde(default)]
    pub last_error_code: String,
}

pub(crate) type AdminAlertingSection = AdminDashboardSection<AdminAlertingData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminCredentialAvailability {
    pub world: String,
    pub available: i64,
    pub invalid: i64,
    pub updated_at: Option<String>,
    pub restart_required: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminCredentialData {
    pub availability: Vec<AdminCredentialAvailability>,
    pub observed_at: String,
    pub last_admin_change_at: Option<String>,
}

pub(crate) type AdminCredentialSection = AdminDashboardSection<AdminCredentialData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminMailData {
    pub observed_at: String,
    pub pollers: Vec<AdminMailPolling>,
}

pub(crate) type AdminMailSection = AdminDashboardSection<AdminMailData>;

/// W5.3 外部账号分区中由既有 owner 投影的子列表状态；`unavailable` 不表示空列表。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalPoller {
    pub mailbox_key: String,
    pub paused: bool,
    pub paused_since: Option<String>,
    pub consecutive_failures: i64,
    pub last_poll_at: String,
    pub next_poll_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalPollerList {
    pub status: String,
    pub items: Vec<AdminExternalPoller>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalMailData {
    pub busy: bool,
    pub busy_age_seconds: Option<i64>,
    pub gate_wait_count: i64,
    pub gate_wait_seconds_total: i64,
    pub request_count: i64,
    pub last_request_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub pollers: AdminExternalPollerList,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalForumData {
    pub last_request_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub write_unknown_count: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalMaintenanceData {
    pub state: String,
    pub last_started_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub last_canceled_at: Option<String>,
    pub running_age_seconds: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalCredential {
    pub world: String,
    pub kind: String,
    pub available: i64,
    pub invalid: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalCredentialList {
    pub status: String,
    pub items: Vec<AdminExternalCredential>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminExternalAccountsData {
    pub observed_at: String,
    pub mail: AdminExternalMailData,
    pub forum: AdminExternalForumData,
    pub maintenance: AdminExternalMaintenanceData,
    pub credentials: AdminExternalCredentialList,
}

pub(crate) type AdminExternalAccountsSection = AdminDashboardSection<AdminExternalAccountsData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminDeliveryWater {
    pub pending: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub oldest_pending_age_seconds: i64,
    pub oldest_manual_reconcile_age_seconds: i64,
    pub last_update_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminDeliveryData {
    pub observed_at: String,
    pub water: AdminDeliveryWater,
}

pub(crate) type AdminDeliverySection = AdminDashboardSection<AdminDeliveryData>;

/// W11.1–W11.3 单个 world 的资产对账差异摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminReconciliationWorld {
    pub world: String,
    pub custody_missing_count: i64,
    pub solvency_shortfall_credits: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminReconciliationData {
    pub observed_at: String,
    pub custody_report_age_seconds: i64,
    pub solvency_report_age_seconds: i64,
    pub worlds: Vec<AdminReconciliationWorld>,
}

pub(crate) type AdminReconciliationSection = AdminDashboardSection<AdminReconciliationData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminFulfillmentData {
    pub pending: i64,
    pub blocked: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub waiting_payment: i64,
    pub waiting_prerequisite: i64,
    pub observed_at: String,
    pub oldest_backlog_age_seconds: i64,
    pub oldest_prerequisite_wait_age_seconds: i64,
}

pub(crate) type AdminFulfillmentSection = AdminDashboardSection<AdminFulfillmentData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRecoveryWorld {
    pub world: String,
    pub balance_credits: i64,
    pub balance_status: String,
    pub evidence_status: String,
    pub balance_observed_at: Option<String>,
    pub evidence_last_success_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdminRecoveryData {
    pub observed_at: String,
    pub worlds: Vec<AdminRecoveryWorld>,
    pub payment_overdue_count: i64,
    pub relist_pending_count: i64,
    pub failed_action_count: i64,
    pub manual_reconcile_count: i64,
    pub oldest_overdue_age_seconds: i64,
}

pub(crate) type AdminRecoverySection = AdminDashboardSection<AdminRecoveryData>;

/// Admin dashboard 全量安全快照；前端只消费这里声明的稳定字段。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminDashboardSnapshot {
    pub health: AdminDashboardHealth,
    pub worker: AdminDashboardWorker,
    pub process: AdminDashboardSection<AdminProcessSectionData>,
    pub mail: AdminMailSection,
    pub realtime: AdminDashboardSection<AdminRealtimeData>,
    pub credentials: AdminCredentialSection,
    pub external_accounts: AdminExternalAccountsSection,
    pub delivery: AdminDeliverySection,
    pub fulfillment: AdminFulfillmentSection,
    pub recovery: AdminRecoverySection,
    pub alerting: AdminAlertingSection,
    pub reconciliation: AdminReconciliationSection,
}

async fn fetch_dashboard_snapshot() -> Result<AdminDashboardSnapshot, String> {
    let response = send_get("/api/v1/admin/dashboard").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<Value> = response.json().await.map_err(invalid_response_error)?;
    decode_dashboard_response(payload.data)
}

fn decode_dashboard_response(value: Value) -> Result<AdminDashboardSnapshot, String> {
    if !valid_dashboard_shape(&value) {
        return Err("invalid_response".to_owned());
    }
    let snapshot: AdminDashboardResponse =
        serde_json::from_value(value).map_err(invalid_response_error)?;
    Ok(AdminDashboardSnapshot {
        health: snapshot.health,
        worker: snapshot.worker,
        process: decode_section(snapshot.process, true)?,
        mail: decode_section(snapshot.mail, true)?,
        realtime: decode_section(snapshot.realtime, true)?,
        credentials: decode_section(snapshot.credentials, true)?,
        external_accounts: decode_section(snapshot.external_accounts, true)?,
        delivery: decode_section(snapshot.delivery, true)?,
        fulfillment: decode_section(snapshot.fulfillment, true)?,
        recovery: decode_section(snapshot.recovery, true)?,
        alerting: decode_section(snapshot.alerting, false)?,
        reconciliation: decode_section(snapshot.reconciliation, true)?,
    })
}

fn decode_section<T: DeserializeOwned>(
    section: AdminDashboardSectionResponse,
    observed_at_required: bool,
) -> Result<AdminDashboardSection<T>, String> {
    let data =
        section.data.map(serde_json::from_value).transpose().map_err(invalid_response_error)?;
    let valid = match section.status.as_str() {
        "unknown" => {
            section.observed_at.is_none()
                && data.is_none()
                && section.error_code.as_deref().is_some_and(|code| !code.is_empty())
        }
        "ok" | "degraded" => {
            section.error_code.is_none()
                && data.is_some()
                && (!observed_at_required || section.observed_at.is_some())
        }
        _ => false,
    };
    if !valid {
        return Err("invalid_response".to_owned());
    }
    Ok(AdminDashboardSection {
        status: section.status,
        observed_at: section.observed_at,
        error_code: section.error_code,
        data,
    })
}

fn valid_dashboard_shape(value: &Value) -> bool {
    let Some(dashboard) = exact_object(
        value,
        &[
            "health",
            "worker",
            "process",
            "mail",
            "realtime",
            "credentials",
            "external_accounts",
            "delivery",
            "fulfillment",
            "recovery",
            "alerting",
            "reconciliation",
        ],
    ) else {
        return false;
    };

    valid_health_shape(&dashboard["health"])
        && exact_object(
            &dashboard["worker"],
            &["condition", "mail_pollers", "ready", "state", "version"],
        )
        .is_some()
        && valid_section(&dashboard["process"], valid_process_shape, true)
        && valid_section(&dashboard["mail"], valid_mail_shape, true)
        && valid_section(&dashboard["realtime"], valid_realtime_shape, true)
        && valid_section(&dashboard["credentials"], valid_credentials_shape, true)
        && valid_section(&dashboard["external_accounts"], valid_external_accounts_shape, true)
        && valid_section(&dashboard["delivery"], valid_delivery_shape, true)
        && valid_section(&dashboard["fulfillment"], valid_fulfillment_shape, true)
        && valid_section(&dashboard["recovery"], valid_recovery_shape, true)
        && valid_alerting_section(&dashboard["alerting"])
        && valid_section(&dashboard["reconciliation"], valid_reconciliation_shape, true)
}

fn valid_reconciliation_shape(value: &Value) -> bool {
    let Some(data) = value.as_object() else {
        return false;
    };
    data.keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>()
        == ["custody_report_age_seconds", "observed_at", "solvency_report_age_seconds", "worlds"]
            .into_iter()
            .collect()
        && data["worlds"].as_array().is_some_and(|worlds| {
            worlds.len() <= 2
                && worlds.iter().all(|world| {
                    exact_object(
                        world,
                        &["custody_missing_count", "solvency_shortfall_credits", "world"],
                    )
                    .is_some()
                })
        })
}

fn valid_section(
    value: &Value,
    valid_data: fn(&Value) -> bool,
    observed_at_required: bool,
) -> bool {
    let Some(section) = value.as_object() else {
        return false;
    };
    match section.get("status").and_then(Value::as_str) {
        Some("unknown") => {
            exact_keys(section, &["error_code", "status"])
                && section
                    .get("error_code")
                    .and_then(Value::as_str)
                    .is_some_and(|code| !code.is_empty())
        }
        Some("ok" | "degraded") => {
            let keys_valid = if observed_at_required {
                exact_keys(section, &["data", "observed_at", "status"])
                    && section.get("observed_at").is_some_and(Value::is_string)
            } else {
                (section.len() == 2 || section.len() == 3)
                    && section.contains_key("data")
                    && section.contains_key("status")
                    && section.get("observed_at").is_none_or(Value::is_string)
            };
            keys_valid && section.get("data").is_some_and(valid_data)
        }
        _ => false,
    }
}

fn valid_external_accounts_shape(value: &Value) -> bool {
    let Some(data) =
        exact_object(value, &["credentials", "forum", "mail", "maintenance", "observed_at"])
    else {
        return false;
    };
    let Some(mail) = exact_object(
        &data["mail"],
        &[
            "busy",
            "busy_age_seconds",
            "gate_wait_count",
            "gate_wait_seconds_total",
            "last_failure_at",
            "last_request_at",
            "last_success_at",
            "pollers",
            "request_count",
        ],
    ) else {
        return false;
    };
    let Some(forum) = exact_object(
        &data["forum"],
        &["last_failure_at", "last_request_at", "last_success_at", "write_unknown_count"],
    ) else {
        return false;
    };
    let Some(maintenance) = exact_object(
        &data["maintenance"],
        &[
            "last_canceled_at",
            "last_failure_at",
            "last_started_at",
            "last_success_at",
            "running_age_seconds",
            "state",
        ],
    ) else {
        return false;
    };
    let busy_age_consistent = match (mail["busy"].as_bool(), mail["busy_age_seconds"].as_i64()) {
        (Some(true), Some(age)) => age >= 0,
        (Some(false), None) => true,
        _ => false,
    };
    let maintenance_state_valid = matches!(
        maintenance["state"].as_str(),
        Some("idle" | "running" | "succeeded" | "failed" | "canceled")
    );
    busy_age_consistent
        && maintenance_state_valid
        && valid_external_projection(&mail["pollers"], |poller| {
            exact_object(
                poller,
                &[
                    "consecutive_failures",
                    "last_poll_at",
                    "mailbox_key",
                    "next_poll_at",
                    "paused",
                    "paused_since",
                ],
            )
            .is_some()
        })
        && valid_external_projection(&data["credentials"], |credential| {
            exact_object(credential, &["available", "invalid", "kind", "world"]).is_some()
        })
        && forum["write_unknown_count"].as_i64().is_some_and(|count| count >= 0)
}

/// 子投影只允许 `available|unavailable`；不可用时空列表不是“没有条目”的事实。
fn valid_external_projection(value: &Value, valid_item: fn(&Value) -> bool) -> bool {
    let Some(projection) = exact_object(value, &["items", "status"]) else {
        return false;
    };
    matches!(projection["status"].as_str(), Some("available" | "unavailable"))
        && projection["items"].as_array().is_some_and(|items| items.iter().all(valid_item))
}

fn valid_health_shape(value: &Value) -> bool {
    exact_object(value, &["audit_failure_count", "missing_services", "readiness", "status"])
        .is_some()
}

fn valid_process_shape(value: &Value) -> bool {
    let Some(data) = exact_object(value, &["observed_at", "processes", "summary"]) else {
        return false;
    };
    data["processes"].as_array().is_some_and(|processes| {
        processes.iter().all(|process| {
            let Some(process) = exact_object(
                process,
                &[
                    "audit_failure_count",
                    "expires_at",
                    "instance_id",
                    "observed_at",
                    "process_role",
                    "readiness",
                    "service_name",
                    "started_at",
                    "status",
                    "tracing",
                    "version",
                ],
            ) else {
                return false;
            };
            exact_object(
                &process["tracing"],
                &[
                    "dropped_span_count",
                    "export_failure_count",
                    "last_error_code",
                    "last_failure_at",
                    "last_success_at",
                    "status",
                ],
            )
            .is_some()
        })
    }) && valid_health_shape(&data["summary"])
}

fn valid_mail_shape(value: &Value) -> bool {
    let Some(data) = exact_object(value, &["observed_at", "pollers"]) else {
        return false;
    };
    data["pollers"].as_array().is_some_and(|pollers| {
        pollers.iter().all(|poller| {
            exact_object(
                poller,
                &[
                    "consecutive_failures",
                    "last_poll_at",
                    "mailbox_key",
                    "next_poll_at",
                    "paused",
                    "state_version",
                    "updated_at",
                ],
            )
            .is_some()
        })
    })
}

fn valid_realtime_shape(value: &Value) -> bool {
    let Some(data) = exact_object(value, &["failure", "lags", "observed_at", "total_lag"]) else {
        return false;
    };
    exact_object(&data["failure"], &["count", "latest_code", "oldest_at"]).is_some()
        && data["total_lag"].as_i64().is_some_and(|total_lag| total_lag >= 0)
        && data["lags"].as_array().is_some_and(|lags| {
            lags.iter().all(|lag| {
                exact_object(
                    lag,
                    &[
                        "consumer_key",
                        "lag",
                        "last_advanced_at",
                        "latest_sequence",
                        "offset",
                        "stream_key",
                    ],
                )
                .is_some()
            })
        })
}

fn valid_credentials_shape(value: &Value) -> bool {
    let Some(data) = exact_object(value, &["availability", "last_admin_change_at", "observed_at"])
    else {
        return false;
    };
    data["availability"].as_array().is_some_and(|availability| {
        availability.iter().all(|item| {
            exact_object(item, &["available", "invalid", "restart_required", "updated_at", "world"])
                .is_some()
        })
    })
}

fn valid_delivery_shape(value: &Value) -> bool {
    let Some(data) = exact_object(value, &["observed_at", "water"]) else {
        return false;
    };
    exact_object(
        &data["water"],
        &[
            "failed",
            "last_update_at",
            "manual_reconcile",
            "oldest_manual_reconcile_age_seconds",
            "oldest_pending_age_seconds",
            "pending",
        ],
    )
    .is_some()
}

fn valid_fulfillment_shape(value: &Value) -> bool {
    exact_object(
        value,
        &[
            "blocked",
            "failed",
            "manual_reconcile",
            "observed_at",
            "oldest_backlog_age_seconds",
            "oldest_prerequisite_wait_age_seconds",
            "pending",
            "waiting_payment",
            "waiting_prerequisite",
        ],
    )
    .is_some()
}

fn valid_recovery_shape(value: &Value) -> bool {
    let Some(data) = exact_object(
        value,
        &[
            "failed_action_count",
            "manual_reconcile_count",
            "observed_at",
            "oldest_overdue_age_seconds",
            "payment_overdue_count",
            "relist_pending_count",
            "worlds",
        ],
    ) else {
        return false;
    };
    data["worlds"].as_array().is_some_and(|worlds| {
        worlds.iter().all(|world| {
            exact_object(
                world,
                &[
                    "balance_credits",
                    "balance_observed_at",
                    "balance_status",
                    "evidence_last_success_at",
                    "evidence_status",
                    "updated_at",
                    "world",
                ],
            )
            .is_some()
        })
    })
}

fn valid_alerting_section(value: &Value) -> bool {
    if !valid_section(value, valid_alerting_shape, false) {
        return false;
    }
    let Some(section) = value.as_object() else {
        return false;
    };
    if section.get("status").and_then(Value::as_str) == Some("unknown") {
        return true;
    }
    let Some(data) = section.get("data").and_then(Value::as_object) else {
        return false;
    };
    if data.get("enabled").and_then(Value::as_bool) == Some(false) {
        return section.get("status").and_then(Value::as_str) == Some("ok")
            && !section.contains_key("observed_at");
    }
    match data.get("last_observed_at").and_then(Value::as_str) {
        Some(observed_at) => {
            section.get("observed_at").and_then(Value::as_str) == Some(observed_at)
        }
        None => !section.contains_key("observed_at"),
    }
}

fn valid_alerting_shape(value: &Value) -> bool {
    exact_object(
        value,
        &[
            "active_signal_count",
            "claimed_batch_count",
            "due_signal_count",
            "enabled",
            "failed_batch_count",
            "last_error_code",
            "last_failure_at",
            "last_observed_at",
            "last_success_at",
            "manual_reconcile_count",
            "succeeded_batch_count",
            "webhook_failed_count",
            "webhook_manual_reconcile_count",
            "webhook_pending_count",
            "webhook_sending_count",
        ],
    )
    .is_some()
}

/// 读取 Admin dashboard 的完整安全快照。
pub(crate) async fn fetch_dashboard() -> Result<AdminDashboardSnapshot, String> {
    fetch_dashboard_snapshot().await
}

#[cfg(test)]
mod tests {
    use super::decode_dashboard_response;
    use serde_json::json;

    fn unknown_section() -> serde_json::Value {
        json!({"status": "unknown", "error_code": "source_unavailable"})
    }

    #[test]
    fn complete_dashboard_payload_with_waiting_and_webhook_fields_decodes() {
        let payload = json!({
            "health": {
                "status": "ok",
                "readiness": true,
                "audit_failure_count": 0,
                "missing_services": []
            },
            "worker": {
                "condition": "available",
                "state": "serving",
                "ready": true,
                "version": "0.1.0",
                "mail_pollers": 1
            },
            "process": unknown_section(),
            "mail": unknown_section(),
            "realtime": unknown_section(),
            "credentials": unknown_section(),
            "external_accounts": {
                "status": "ok",
                "observed_at": "2026-01-01T00:00:00Z",
                "data": {
                    "observed_at": "2026-01-01T00:00:00Z",
                    "mail": {
                        "busy": true,
                        "busy_age_seconds": 12,
                        "gate_wait_count": 2,
                        "gate_wait_seconds_total": 3,
                        "request_count": 5,
                        "last_request_at": "2026-01-01T00:00:00Z",
                        "last_success_at": null,
                        "last_failure_at": null,
                        "pollers": {
                            "status": "available",
                            "items": [{
                                "mailbox_key": "main",
                                "paused": true,
                                "paused_since": "2026-01-01T00:00:00Z",
                                "consecutive_failures": 0,
                                "last_poll_at": "2026-01-01T00:00:00Z",
                                "next_poll_at": "2026-01-01T00:01:00Z"
                            }]
                        }
                    },
                    "forum": {
                        "last_request_at": null,
                        "last_success_at": null,
                        "last_failure_at": null,
                        "write_unknown_count": 1
                    },
                    "maintenance": {
                        "state": "running",
                        "last_started_at": "2026-01-01T00:00:00Z",
                        "last_success_at": null,
                        "last_failure_at": null,
                        "last_canceled_at": null,
                        "running_age_seconds": 4
                    },
                    "credentials": {
                        "status": "available",
                        "items": [
                            {
                                "world": "main",
                                "kind": "mail_cookie",
                                "available": 1,
                                "invalid": 0
                            }
                        ]
                    }
                }
            },
            "delivery": unknown_section(),
            "recovery": unknown_section(),
            "fulfillment": {
                "status": "ok",
                "observed_at": "2026-01-01T00:00:00Z",
                "data": {
                    "pending": 2,
                    "blocked": 1,
                    "failed": 0,
                    "manual_reconcile": 0,
                    "waiting_payment": 4,
                    "waiting_prerequisite": 3,
                    "observed_at": "2026-01-01T00:00:00Z",
                    "oldest_backlog_age_seconds": 120,
                    "oldest_prerequisite_wait_age_seconds": 60
                }
            },
            "alerting": {
                "status": "ok",
                "observed_at": "2026-01-01T00:00:00Z",
                "data": {
                    "enabled": true,
                    "active_signal_count": 1,
                    "due_signal_count": 0,
                    "claimed_batch_count": 0,
                    "succeeded_batch_count": 1,
                    "failed_batch_count": 0,
                    "manual_reconcile_count": 0,
                    "webhook_pending_count": 2,
                    "webhook_sending_count": 0,
                    "webhook_failed_count": 0,
                    "webhook_manual_reconcile_count": 1,
                    "last_observed_at": "2026-01-01T00:00:00Z",
                    "last_success_at": null,
                    "last_failure_at": null,
                    "last_error_code": ""
                }
            },
            "reconciliation": unknown_section()
        });

        let snapshot = decode_dashboard_response(payload).expect("完整响应必须通过浏览器解码");
        let fulfillment = snapshot.fulfillment.data.expect("履约数据");
        assert_eq!(fulfillment.waiting_payment, 4);
        assert_eq!(fulfillment.waiting_prerequisite, 3);
        assert_eq!(fulfillment.oldest_prerequisite_wait_age_seconds, 60);
        let alerting = snapshot.alerting.data.expect("告警数据");
        assert_eq!(alerting.webhook_pending_count, 2);
        assert_eq!(alerting.webhook_manual_reconcile_count, 1);
        let external = snapshot.external_accounts.data.expect("外部账号数据");
        assert!(external.mail.busy);
        assert_eq!(external.mail.busy_age_seconds, Some(12));
        assert!(external.mail.pollers.items[0].paused);
        assert_eq!(external.maintenance.state, "running");
        assert_eq!(external.forum.write_unknown_count, 1);
        assert_eq!(external.credentials.items[0].available, 1);
    }

    #[test]
    fn unknown_external_accounts_section_decodes_without_data() {
        let payload = json!({
            "health": {
                "status": "ok",
                "readiness": true,
                "audit_failure_count": 0,
                "missing_services": []
            },
            "worker": {
                "condition": "unavailable",
                "state": "",
                "ready": false,
                "version": "",
                "mail_pollers": 0
            },
            "process": unknown_section(),
            "mail": unknown_section(),
            "realtime": unknown_section(),
            "credentials": unknown_section(),
            "external_accounts": unknown_section(),
            "delivery": unknown_section(),
            "fulfillment": unknown_section(),
            "recovery": unknown_section(),
            "alerting": unknown_section(),
            "reconciliation": unknown_section()
        });

        let snapshot = decode_dashboard_response(payload).expect("不可用分区仍须解码");
        assert!(snapshot.external_accounts.data.is_none());
        assert_eq!(snapshot.external_accounts.status, "unknown");
        assert_eq!(snapshot.external_accounts.error_code.as_deref(), Some("source_unavailable"));
    }
}
