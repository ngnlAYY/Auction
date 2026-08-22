//! Admin delivery HTTP 数据访问。
//!
//! 浏览器只接收后端的安全 delivery 投影。人工动作携带新的 `command_id` 和页面快照中的
//! `expected_fencing_version`；服务端仍负责最终的状态校验、幂等和审计。

mod auctions;
mod intake;
mod lifecycle;
mod lots;
mod mail;
mod proxy_events;
mod realtime;
mod renames;
mod rules;
mod runtime_config;

pub(crate) use auctions::{
    AdminAuction, AdminAuctionPage, create_admin_auction, fetch_admin_auctions,
};
pub(crate) use intake::{AdminIntakeResult, add_admin_intake};
pub(crate) use lifecycle::{AdminAuctionLifecycleAction, execute_admin_auction_lifecycle};
pub(crate) use lots::{
    AdminLot, AdminLotPage, AdminLotUpdateAction, fetch_admin_lots, update_admin_lot,
};
pub(crate) use mail::{
    AdminMailIntake, AdminMailIntakePage, AdminMailPolling, fetch_mail_intakes, fetch_mail_state,
    set_mail_paused, trigger_mail_poll,
};
pub(crate) use proxy_events::{AdminProxyEvent, AdminProxyEventPage, fetch_admin_proxy_events};
pub(crate) use realtime::{
    RealtimeFailure, RealtimeFailurePage, RealtimeLagPage, fetch_realtime_failures,
    fetch_realtime_lag, replay_realtime_failure,
};
pub(crate) use renames::{AdminRename, AdminRenamePage, fetch_admin_renames};
pub(crate) use rules::{AdminRules, fetch_admin_rules, publish_admin_rules, validate_admin_rules};
pub(crate) use runtime_config::{
    fetch_admin_runtime_config, update_admin_runtime_config, validate_admin_runtime_config,
};

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
struct ApiResponse<T> {
    data: T,
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
    pub status: String,
    pub lease_expires_at: Option<String>,
    pub fencing_version: i64,
    pub attempt_count: u32,
    pub claimed_at: Option<String>,
    pub sending_at: Option<String>,
    pub completed_at: Option<String>,
    pub external_reference: Option<String>,
    pub failure_code: Option<String>,
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
}

impl DeliveryAction {
    pub(crate) const fn path(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Retry => "retry",
            Self::ReconcileRetry => "reconcile",
        }
    }

    pub(crate) const fn request_action(self) -> Option<&'static str> {
        match self {
            Self::Release | Self::Retry => None,
            Self::ReconcileRetry => Some("retry"),
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

pub(crate) async fn fetch_deliveries() -> Result<DeliveryPage, String> {
    let response = Request::get("/api/v1/admin/deliveries?page=1&page_size=50")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<DeliveryListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

pub(crate) async fn execute_delivery(
    delivery_id: String,
    action: DeliveryAction,
    expected_fencing_version: i64,
) -> Result<Delivery, String> {
    let command = DeliveryCommandRequest {
        command_id: Uuid::new_v4(),
        expected_fencing_version,
        action: action.request_action(),
        external_reference: None,
        failure_code: None,
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
