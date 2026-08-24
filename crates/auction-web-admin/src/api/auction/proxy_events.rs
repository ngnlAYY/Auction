//! Admin 代理历史 API 客户端。
//!
//! 浏览器只解码后端的安全代理事件投影；游标、命令账本结果和代理状态机仍由服务端拥有。

use gloo_net::http::Request;
use serde::Deserialize;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminProxyEvent {
    pub authorization_id: String,
    pub authorization_sequence: i64,
    pub command_id: String,
    pub created_at: String,
    pub lot_id: String,
    pub maximum_amount: i64,
    pub mode: String,
    pub state_version: i64,
    pub status: String,
    pub user_id: String,
    pub usage_number: i64,
    pub username: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminProxyEventPagination {
    pub page: Option<i64>,
    pub page_size: i64,
    pub total: Option<i64>,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub has_more: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminProxyEventListData {
    proxy_events: Vec<AdminProxyEvent>,
    pagination: AdminProxyEventPagination,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminProxyEventPage {
    pub proxy_events: Vec<AdminProxyEvent>,
    pub pagination: AdminProxyEventPagination,
}

impl From<AdminProxyEventListData> for AdminProxyEventPage {
    fn from(data: AdminProxyEventListData) -> Self {
        Self { proxy_events: data.proxy_events, pagination: data.pagination }
    }
}

pub(crate) async fn fetch_admin_proxy_events(
    lot_id: String,
    page: i64,
) -> Result<AdminProxyEventPage, String> {
    let url = format!("/api/v1/admin/lots/{lot_id}/proxy-events?page={page}&page_size=50");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminProxyEventListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}
