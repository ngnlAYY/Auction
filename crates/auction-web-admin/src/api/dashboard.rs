//! Admin dashboard 聚合投影客户端。
//!
//! 页面只读取各 owner 提供的安全摘要；数据库行、delivery payload、Cookie 和外部错误正文
//! 不会进入浏览器。DTO 与后端 dashboard 响应保持字段闭集，便于旧版运维信息逐项迁移。

use gloo_net::http::Request;
use serde::Deserialize;

use super::ApiResponse;
use super::mail::AdminMailPolling;

fn no_data<T>() -> Option<T> {
    None
}

/// Admin dashboard 各 owner section 的稳定安全外壳。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminDashboardSection<T> {
    pub status: String,
    #[serde(default)]
    pub observed_at: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default = "no_data")]
    pub data: Option<T>,
}

/// Admin dashboard 顶层健康摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminDashboardHealth {
    pub status: String,
    pub readiness: bool,
    pub audit_failure_count: i64,
    pub missing_services: Vec<String>,
}

/// Admin dashboard 的 Worker control 摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminDashboardWorker {
    pub condition: String,
    pub state: String,
    pub ready: bool,
    pub version: String,
    pub mail_pollers: i64,
}

/// 进程 tracing 安全摘要。
#[derive(Debug, Clone, Deserialize, PartialEq)]
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
pub(crate) struct AdminProcessSectionData {
    pub processes: Vec<AdminProcessData>,
    pub summary: AdminDashboardHealth,
    pub missing_services: Vec<String>,
    pub observed_at: Option<String>,
}

/// Realtime dashboard 的 lag 条目。
#[derive(Debug, Clone, Deserialize, PartialEq)]
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
pub(crate) struct AdminRealtimeFailure {
    pub count: i64,
    pub oldest_at: Option<String>,
    pub latest_code: String,
}

/// Realtime dashboard section 的数据。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRealtimeData {
    pub lags: Vec<AdminRealtimeLag>,
    pub failure: AdminRealtimeFailure,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminAlertingData {
    pub enabled: bool,
    pub active_signal_count: i64,
    pub due_signal_count: i64,
    pub claimed_batch_count: i64,
    pub succeeded_batch_count: i64,
    pub failed_batch_count: i64,
    pub manual_reconcile_count: i64,
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
pub(crate) struct AdminCredentialScope {
    pub kind: String,
    pub world: String,
    pub active_count: i64,
    pub disabled_count: i64,
    pub invalid_count: i64,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub restart_required: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminCredentialData {
    pub active_count: i64,
    pub disabled_count: i64,
    pub invalid_count: i64,
    #[serde(default)]
    pub last_admin_change_at: Option<String>,
    #[serde(default)]
    pub restart_required: Option<bool>,
    #[serde(default)]
    pub scopes: Vec<AdminCredentialScope>,
}

pub(crate) type AdminCredentialSection = AdminDashboardSection<AdminCredentialData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminMailData {
    pub pollers: Vec<AdminMailPolling>,
}

pub(crate) type AdminMailSection = AdminDashboardSection<AdminMailData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminDeliveryWater {
    pub pending: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub oldest_pending_age_seconds: i64,
    pub oldest_manual_reconcile_age_seconds: i64,
    pub last_update_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminDeliveryData {
    pub water: AdminDeliveryWater,
}

pub(crate) type AdminDeliverySection = AdminDashboardSection<AdminDeliveryData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminFulfillmentData {
    pub pending: i64,
    pub blocked: i64,
    pub failed: i64,
    pub manual_reconcile: i64,
    pub oldest_backlog_age_seconds: i64,
}

pub(crate) type AdminFulfillmentSection = AdminDashboardSection<AdminFulfillmentData>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRecoveryWorld {
    pub world: String,
    pub balance_credits: i64,
    pub balance_status: String,
    pub evidence_status: String,
    pub evidence_age_seconds: Option<i64>,
    pub balance_observed_at: Option<String>,
    pub evidence_last_success_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminRecoveryData {
    pub worlds: Vec<AdminRecoveryWorld>,
    pub payment_overdue_count: i64,
    pub relist_pending_count: i64,
    pub failed_action_count: i64,
    pub manual_reconcile_count: i64,
    pub oldest_overdue_age_seconds: i64,
}

pub(crate) type AdminRecoverySection = AdminDashboardSection<AdminRecoveryData>;

/// Admin dashboard 全量安全快照；前端只消费这里声明的稳定字段。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminDashboardSnapshot {
    pub health: AdminDashboardHealth,
    pub worker: AdminDashboardWorker,
    pub process: AdminDashboardSection<AdminProcessSectionData>,
    pub mail: AdminMailSection,
    pub realtime: AdminDashboardSection<AdminRealtimeData>,
    pub credentials: AdminCredentialSection,
    pub delivery: AdminDeliverySection,
    pub fulfillment: AdminFulfillmentSection,
    pub recovery: AdminRecoverySection,
    pub alerting: AdminAlertingSection,
}

async fn fetch_dashboard_snapshot() -> Result<AdminDashboardSnapshot, String> {
    let response =
        Request::get("/api/v1/admin/dashboard").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminDashboardSnapshot> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

/// 读取 Admin dashboard 的完整安全快照。
pub(crate) async fn fetch_dashboard() -> Result<AdminDashboardSnapshot, String> {
    fetch_dashboard_snapshot().await
}
