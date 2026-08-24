//! Public 账户读取 DTO 与同源请求。
//!
//! 这里只解码后端的安全账户 projection；代理上限、历史转换和 capability 仍以服务端返回的
//! 公开字段为准，浏览器不重建代理状态机。

use serde::Deserialize;

use super::send_get;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct OwnedEquipment {
    pub name: String,
    pub category: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct OwnedBid {
    pub id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub amount: i64,
    pub accepted_state_version: i64,
    pub fact_index: i32,
    pub mechanism: String,
    pub created_at: String,
    pub leading: bool,
    pub equipment: OwnedEquipment,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct ProxyQuota {
    pub used: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct ProxyTransition {
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
pub(crate) struct ProxyCapabilities {
    pub can_lower: bool,
    pub waiting: bool,
    pub at_maximum: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct OwnedProxy {
    pub authorization_id: String,
    pub auction_id: String,
    pub lot_id: String,
    pub lot_number: i64,
    pub authorization_sequence: i64,
    pub usage_number: i64,
    pub mode: String,
    pub maximum_amount: i64,
    pub quota: ProxyQuota,
    pub transitions: Vec<ProxyTransition>,
    pub status: String,
    pub terminal_reason: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub equipment: OwnedEquipment,
    pub bid: Option<OwnedBid>,
    pub invalid_reason: Option<String>,
    pub capabilities: ProxyCapabilities,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct MyBidsPage {
    pub bids: Vec<OwnedBid>,
    pub proxies: Vec<OwnedProxy>,
    pub bid_total: i64,
    pub proxy_total: i64,
    pub page: i64,
    pub bid_page: i64,
    pub proxy_page: i64,
    pub page_size: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Debug, Clone, Deserialize)]
struct MyProxyData {
    authorization: Option<OwnedProxy>,
}

pub(crate) async fn fetch_my_bids(bid_page: i64, proxy_page: i64) -> Result<MyBidsPage, String> {
    let url = format!("/api/v1/me/bids?bid_page={bid_page}&proxy_page={proxy_page}&page_size=20");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<MyBidsPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_my_proxy(lot_id: String) -> Result<Option<OwnedProxy>, String> {
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations/mine");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<MyProxyData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.authorization)
}
