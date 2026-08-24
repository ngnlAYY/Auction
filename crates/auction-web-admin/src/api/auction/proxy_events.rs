//! Admin 代理历史 API 客户端。
//!
//! 浏览器只解码后端的安全代理事件投影；游标、命令账本结果和代理状态机仍由服务端拥有。

pub(crate) use auction_contracts::{
    AdminProxyEventListResponse as AdminProxyEventPage, AdminProxyEventResponse as AdminProxyEvent,
};
use gloo_net::http::Request;

use super::ApiResponse;

pub(crate) async fn fetch_admin_proxy_events(
    lot_id: String,
    page: i64,
) -> Result<AdminProxyEventPage, String> {
    let url = format!("/api/v1/admin/lots/{lot_id}/proxy-events?page={page}&page_size=50");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminProxyEventPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
