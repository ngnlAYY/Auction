//! Admin delivery HTTP 数据访问。
//!
//! 浏览器只接收后端的安全 delivery 投影。人工动作携带由页面按输入复用的 `command_id`
//! 和页面快照中的 `expected_fencing_version`；服务端仍负责最终的状态校验、幂等和审计。

mod auctions;
mod audit;
mod credentials;
mod dashboard;
mod intake;
mod lifecycle;
mod lots;
mod mail;
mod proxy_events;
mod realtime;
mod recovery;
mod renames;
mod rules;
mod runtime_config;
mod session;
mod settlement;
mod status;
mod users;

pub(crate) use auctions::{
    AdminAuction, AdminAuctionCapabilities, AdminAuctionPage, create_admin_auction,
    fetch_admin_auctions,
};
pub(crate) use audit::{AdminAuditFilter, AdminAuditPage, AdminAuditRecord, fetch_admin_audit};
pub(crate) use credentials::{
    AdminCredential, AdminCredentialPage, create_admin_credential, delete_admin_credential,
    fetch_admin_credentials, mutate_admin_credential,
};
pub(crate) use dashboard::{
    AdminAlertingSection, AdminCredentialData, AdminCredentialScope, AdminCredentialSection,
    AdminDashboardSnapshot, AdminMailData, AdminMailSection, AdminRealtimeData, fetch_dashboard,
};
pub(crate) use intake::{AdminIntakeResult, add_admin_intake};
pub(crate) use lifecycle::{AdminAuctionLifecycleAction, execute_admin_auction_lifecycle};
pub(crate) use lots::{
    AdminLot, AdminLotPage, AdminLotUpdateAction, fetch_admin_lots, update_admin_lot,
};
pub(crate) use mail::{
    AdminMailIntake, AdminMailIntakeDetail, AdminMailIntakePage, AdminMailIntakeQuery,
    AdminMailPolling, fetch_mail_intake_detail, fetch_mail_intakes, fetch_mail_state,
    set_mail_paused, trigger_mail_poll,
};
pub(crate) use proxy_events::{AdminProxyEvent, AdminProxyEventPage, fetch_admin_proxy_events};
pub(crate) use realtime::{
    RealtimeFailure, RealtimeFailurePage, RealtimeLagPage, fetch_realtime_failure_page,
    fetch_realtime_lag_page, replay_realtime_failure,
};
pub(crate) use recovery::{
    AdminRecovery, AdminRecoveryAction, AdminRecoveryReport, confirm_admin_recovery,
    fetch_admin_recovery,
};
pub(crate) use renames::{AdminRename, AdminRenamePage, fetch_admin_renames, revert_admin_rename};
pub(crate) use rules::{
    AdminRules, AdminRulesCandidate, fetch_admin_rules, publish_admin_rules, validate_admin_rules,
};
pub(crate) use runtime_config::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};
pub(crate) use session::{AdminSession, fetch_admin_session, login_admin, logout_admin};
pub(crate) use settlement::{
    AdminSettlementLot, AdminSettlementReport, AdminSettlementSummary, fetch_admin_settlement,
};
pub(crate) use status::{
    AdminHealthProbeState, AdminReadinessState, fetch_admin_health, fetch_admin_readiness,
};
pub(crate) use users::{
    AdminUser, AdminUserPage, AdminUserStatusFilter, blacklist_admin_user, create_admin_user,
    fetch_admin_users, mutate_admin_user, unblacklist_admin_user,
};

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

/// 将浏览器 `datetime-local` 按本地时区转换为 API 使用的 UTC 文本。
///
/// Admin 的创建和筛选必须共享这一转换，否则非 UTC 管理员提交的时间范围会产生偏移。
pub(crate) fn normalize_datetime(value: &str) -> String {
    if value.is_empty()
        || value.ends_with('Z')
        || value.contains('+')
        || value.get(10..).is_some_and(|tail| tail.contains('-'))
    {
        return value.to_owned();
    }

    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        if date.get_time().is_finite() {
            return date.to_iso_string().into();
        }
    }

    // 非 WASM 构建只用于静态检查；保留服务端可识别的稳定兜底格式。
    if value.len() == 16 { format!("{value}:00Z") } else { format!("{value}Z") }
}

#[derive(Debug, Clone, Deserialize)]
struct AdminSessionData {
    csrf_token: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct Delivery {
    pub id: String,
    pub origin_kind: String,
    pub origin_id: String,
    pub recipient: String,
    pub cod: Option<i64>,
    pub status: String,
    pub lease_expires_at: Option<String>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub claimed_at: Option<String>,
    pub sending_at: Option<String>,
    pub completed_at: Option<String>,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
    #[serde(default)]
    pub reason_code: Option<String>,
    #[serde(default)]
    pub take_required: bool,
    #[serde(default)]
    pub returned_mail_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct DeliveryListData {
    deliveries: Vec<Delivery>,
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct DeliveryPage {
    pub deliveries: Vec<Delivery>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

impl From<DeliveryListData> for DeliveryPage {
    fn from(data: DeliveryListData) -> Self {
        Self {
            deliveries: data.deliveries,
            page: data.page,
            page_size: data.page_size,
            total: data.total,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryAction {
    Release,
    Retry,
    ReconcileRetry,
    ReconcileSucceeded,
    ReconcileFailed,
}

impl DeliveryAction {
    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Retry => "retry",
            Self::ReconcileRetry | Self::ReconcileSucceeded | Self::ReconcileFailed => "reconcile",
        }
    }

    pub(crate) const fn request_action(self) -> Option<&'static str> {
        match self {
            Self::Release | Self::Retry => None,
            Self::ReconcileRetry => Some("retry"),
            Self::ReconcileSucceeded => Some("succeeded"),
            Self::ReconcileFailed => Some("failed"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct DeliveryCommandRequest {
    command_id: Uuid,
    expected_fencing_version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeliveryCommandData {
    delivery: Delivery,
}

/// 读取指定状态和页码的 delivery 安全投影；状态筛选改变时回到第一页。
pub(crate) async fn fetch_deliveries(status: String, page: i64) -> Result<DeliveryPage, String> {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string()).append_pair("page_size", "50");
    if !status.is_empty() {
        query.append_pair("status", status.trim());
    }
    let url = format!("/api/v1/admin/deliveries?{}", query.finish());
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<DeliveryListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

pub(crate) async fn execute_delivery(
    command_id: Uuid,
    delivery_id: String,
    action: DeliveryAction,
    expected_fencing_version: i64,
    external_reference: Option<String>,
    failure_code: Option<String>,
) -> Result<Delivery, String> {
    let command = DeliveryCommandRequest {
        command_id,
        expected_fencing_version,
        action: action.request_action(),
        external_reference,
        failure_code,
    };
    let url = format!("/api/v1/admin/deliveries/{delivery_id}/{}", action.path());
    let csrf_token = fetch_csrf_token().await?;
    let request = Request::post(&url)
        .header("x-csrf-token", &csrf_token)
        .json(&command)
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<DeliveryCommandData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.delivery)
}

async fn fetch_csrf_token() -> Result<String, String> {
    let response =
        Request::get("/api/v1/admin/session").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminSessionData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.csrf_token.is_empty() {
        return Err("CSRF token unavailable".to_owned());
    }
    Ok(payload.data.csrf_token)
}
