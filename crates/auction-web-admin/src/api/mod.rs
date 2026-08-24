//! Admin 浏览器端的同源 HTTP 数据访问。
//!
//! 浏览器只接收后端安全投影。人工动作携带由页面按输入复用的 `command_id` 和对应版本 fence；
//! 服务端仍负责最终的状态校验、幂等和审计。所有 wire DTO 统一由 `auction-contracts` 拥有，
//! 本 crate 只保留浏览器表单、筛选和交互状态。

#[path = "auction/auctions.rs"]
mod auctions;
#[path = "operations/audit.rs"]
mod audit;
#[path = "identity/credentials.rs"]
mod credentials;
#[path = "operations/dashboard.rs"]
mod dashboard;
#[path = "auction/intake.rs"]
mod intake;
#[path = "auction/lifecycle.rs"]
mod lifecycle;
#[path = "auction/lots.rs"]
mod lots;
#[path = "fulfillment/mail.rs"]
mod mail;
#[path = "auction/proxy_events.rs"]
mod proxy_events;
#[path = "operations/realtime.rs"]
mod realtime;
#[path = "auction/recovery.rs"]
mod recovery;
#[path = "identity/renames.rs"]
mod renames;
#[path = "content/rules.rs"]
mod rules;
#[path = "operations/runtime_config.rs"]
mod runtime_config;
#[path = "identity/session.rs"]
mod session;
#[path = "auction/settlement.rs"]
mod settlement;
#[path = "operations/status.rs"]
mod status;
#[path = "identity/users.rs"]
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

use auction_contracts::{
    AdminDeliveryCommandRequest, AdminDeliveryCommandResponse, AdminSessionResponse, ApiResponse,
};
pub(crate) use auction_contracts::{
    AdminDeliveryListResponse as DeliveryPage, AdminDeliveryResponse as Delivery,
};
use gloo_net::http::Request;
use uuid::Uuid;

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
    let payload: ApiResponse<DeliveryPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn execute_delivery(
    command_id: Uuid,
    delivery_id: String,
    action: DeliveryAction,
    expected_fencing_version: i64,
    external_reference: Option<String>,
    failure_code: Option<String>,
) -> Result<Delivery, String> {
    let command = AdminDeliveryCommandRequest {
        command_id,
        expected_fencing_version,
        action: action.request_action().map(str::to_owned),
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
    let payload: ApiResponse<AdminDeliveryCommandResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.delivery)
}

async fn fetch_csrf_token() -> Result<String, String> {
    let response =
        Request::get("/api/v1/admin/session").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminSessionResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.csrf_token.is_empty() {
        return Err("CSRF token unavailable".to_owned());
    }
    Ok(payload.data.csrf_token)
}
