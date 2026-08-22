//! Public 状态命令的 HTTP 请求。
//!
//! 浏览器只生成稳定重试所需的 `command_id` 并提交契约 DTO；金额、资格、锁和幂等结果仍由
//! 服务端应用端口决定。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct BidResult {
    pub lot_id: String,
    pub current_price: i64,
    pub effective_end_at: String,
    pub state_version: i64,
    pub leading: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct ProxyCommandQuota {
    pub used: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct ProxyCommandTransition {
    pub transition_number: i64,
    pub kind: String,
    pub old_maximum_amount: Option<i64>,
    pub new_maximum_amount: i64,
    pub old_mode: Option<String>,
    pub new_mode: String,
    pub old_status: Option<String>,
    pub new_status: String,
    pub reason: Option<String>,
    pub resulting_state_version: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct ProxyCommandBid {
    pub lot_id: String,
    pub current_price: i64,
    pub effective_end_at: String,
    pub state_version: i64,
    pub leading: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct ProxyCommandAuthorization {
    pub authorization_id: String,
    pub lot_id: String,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub mode: String,
    pub maximum_amount: i64,
    pub quota: ProxyCommandQuota,
    pub transitions: Vec<ProxyCommandTransition>,
    pub status: String,
    pub terminal_reason: Option<String>,
    pub state_version: i64,
    pub bid: Option<ProxyCommandBid>,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Debug, Clone, Deserialize)]
struct PlaceBidData {
    bid: BidResult,
}

#[derive(Debug, Clone, Deserialize)]
struct ProxyCommandData {
    authorization: ProxyCommandAuthorization,
}

#[derive(Debug, Clone, Serialize)]
struct PlaceBidRequest {
    command_id: Uuid,
    amount: i64,
}

pub(crate) async fn place_bid(lot_id: String, amount: i64) -> Result<BidResult, String> {
    let url = format!("/api/v1/lots/{lot_id}/bids");
    let request = Request::post(&url)
        .json(&PlaceBidRequest { command_id: Uuid::new_v4(), amount })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<PlaceBidData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.bid)
}

/// 创建代理授权并解码旧契约要求的完整命令投影。
pub(crate) async fn create_proxy_authorization(
    lot_id: String,
    maximum_amount: i64,
    mode: String,
) -> Result<ProxyCommandAuthorization, String> {
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations");
    let request = Request::post(&url)
        .json(&ProxyCommandRequest { command_id: Uuid::new_v4(), maximum_amount, mode })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<ProxyCommandData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.authorization)
}

/// 降低代理授权上限并解码旧契约要求的完整命令投影。
pub(crate) async fn lower_proxy_authorization(
    lot_id: String,
    authorization_id: String,
    maximum_amount: i64,
) -> Result<ProxyCommandAuthorization, String> {
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations/{authorization_id}/lower");
    let request = Request::post(&url)
        .json(&LowerProxyCommandRequest { command_id: Uuid::new_v4(), maximum_amount })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<ProxyCommandData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.authorization)
}

#[derive(Debug, Clone, Serialize)]
struct ProxyCommandRequest {
    command_id: Uuid,
    maximum_amount: i64,
    mode: String,
}

#[derive(Debug, Clone, Serialize)]
struct LowerProxyCommandRequest {
    command_id: Uuid,
    maximum_amount: i64,
}
