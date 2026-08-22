//! Public 持久化实时事件 API 客户端。
//!
//! 浏览器只解码经过服务端事件注册表校验的安全信封；`next_after` 和
//! `refetch_required` 原样保留，避免在客户端猜测事件序列或自行修复状态。

#[cfg(target_arch = "wasm32")]
use futures_util::StreamExt;
use gloo_net::http::Request;
use serde::Deserialize;
use serde_json::Value;

use super::ApiResponse;

#[cfg(target_arch = "wasm32")]
use gloo_net::websocket::{Message, futures::WebSocket};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct HealthStatus {
    pub service: String,
    pub status: String,
    pub version: String,
}

pub(crate) async fn fetch_health() -> Result<HealthStatus, String> {
    let response =
        Request::get("/api/v1/health").send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    response.json().await.map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RealtimeEvent {
    pub event_id: String,
    pub stream_key: String,
    pub stream_sequence: i64,
    pub kind: String,
    pub schema_version: i32,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct RealtimeData {
    events: Vec<RealtimeEvent>,
    next_after: i64,
    has_more: bool,
    refetch_required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RealtimePage {
    pub events: Vec<RealtimeEvent>,
    pub next_after: i64,
    pub has_more: bool,
    pub refetch_required: bool,
}

impl From<RealtimeData> for RealtimePage {
    fn from(data: RealtimeData) -> Self {
        Self {
            events: data.events,
            next_after: data.next_after,
            has_more: data.has_more,
            refetch_required: data.refetch_required,
        }
    }
}

pub(crate) async fn fetch_realtime_events(after: i64) -> Result<RealtimePage, String> {
    let url = format!("/api/v1/events?after={after}&page_size=50");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<RealtimeData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

/// 订阅 Public 持久化实时流，并把服务端事件页交给页面替换当前安全快照。
///
/// 浏览器 WebSocket 自动携带同源 HttpOnly session Cookie；匿名客户端由服务端拒绝，页面
/// 仍可使用上面的 HTTP catch-up。客户端不合并事件、不推断状态，只转发完整事件页。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn subscribe_realtime<F>(after: i64, mut on_page: F) -> Result<(), String>
where
    F: FnMut(RealtimePage),
{
    let url = websocket_url(after)?;
    let socket = WebSocket::open(&url).map_err(|error| error.to_string())?;
    let (_write, mut read) = socket.split();
    let mut expected_after = after;
    while let Some(message) = read.next().await {
        let message = message.map_err(|error| error.to_string())?;
        let Message::Text(message) = message else {
            return Err("实时 WebSocket 返回了不支持的二进制消息".to_owned());
        };
        let page = decode_realtime_page(&message)?;
        if page.next_after < expected_after {
            return Err("实时 WebSocket 游标向后移动".to_owned());
        }
        expected_after = page.next_after;
        let refetch_required = page.refetch_required;
        on_page(page);
        if refetch_required {
            return Ok(());
        }
    }
    Err("实时 WebSocket 已关闭".to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn subscribe_realtime<F>(_after: i64, _on_page: F) -> Result<(), String>
where
    F: FnMut(RealtimePage),
{
    Err("实时 WebSocket 仅在 wasm32 浏览器目标可用".to_owned())
}

#[cfg(target_arch = "wasm32")]
fn websocket_url(after: i64) -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "浏览器窗口不可用".to_owned())?;
    let location = window.location();
    let protocol = location.protocol().map_err(|_| "读取浏览器协议失败".to_owned())?;
    let host = location.host().map_err(|_| "读取浏览器主机失败".to_owned())?;
    let websocket_protocol = match protocol.as_str() {
        "http:" => "ws:",
        "https:" => "wss:",
        _ => return Err("浏览器协议不支持 WebSocket".to_owned()),
    };
    Ok(format!("{websocket_protocol}//{host}/api/v1/ws?scope=auctions&after={after}&page_size=50"))
}

#[cfg(target_arch = "wasm32")]
fn decode_realtime_page(message: &str) -> Result<RealtimePage, String> {
    let payload: ApiResponse<RealtimeData> =
        serde_json::from_str(message).map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}
