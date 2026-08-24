//! Public 持久化实时事件 API 客户端。
//!
//! 浏览器只解码经过服务端事件注册表校验的安全信封；`next_after` 和
//! `refetch_required` 原样保留，避免在客户端猜测事件序列或自行修复状态。

use auction_contracts::ApiResponse;
pub(crate) use auction_contracts::{
    HealthResponse as HealthStatus, PublicRealtimeEventResponse as RealtimeEvent,
    PublicRealtimeResponse as RealtimePage,
};
use chrono::DateTime;
#[cfg(target_arch = "wasm32")]
use futures_util::StreamExt;
use serde_json::Value;
#[cfg(target_arch = "wasm32")]
use url::form_urlencoded;
use uuid::Uuid;

use super::send_get;

#[cfg(target_arch = "wasm32")]
use gloo_net::websocket::{Message, futures::WebSocket};

pub(crate) async fn fetch_health() -> Result<HealthStatus, String> {
    let response = send_get("/api/v1/health").await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    response.json().await.map_err(|error| error.to_string())
}

pub(crate) async fn fetch_realtime_events(after: i64) -> Result<RealtimePage, String> {
    let url = format!("/api/v1/events?after={after}&page_size=50");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<RealtimePage> =
        response.json().await.map_err(|error| error.to_string())?;
    let page = payload.data;
    validate_realtime_page(&page, after, "auction:discovery")?;
    Ok(page)
}

/// 订阅 Public 持久化实时流，并把服务端事件页交给页面替换当前安全快照。
///
/// 浏览器 WebSocket 自动携带同源 HttpOnly session Cookie；匿名客户端由服务端拒绝，页面
/// 仍可使用上面的 HTTP catch-up。客户端不合并事件、不推断状态，只转发完整事件页。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn subscribe_realtime<F>(after: i64, on_page: F) -> Result<(), String>
where
    F: FnMut(RealtimePage),
{
    subscribe_realtime_stream(after, None, on_page).await
}

/// 订阅单个拍卖的实时流；拍卖页面用它对齐旧版的 lot/竞价即时刷新边界。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn subscribe_auction_realtime<F>(
    after: i64,
    auction_id: String,
    on_page: F,
) -> Result<(), String>
where
    F: FnMut(RealtimePage),
{
    subscribe_realtime_stream(after, Some(&auction_id), on_page).await
}

#[cfg(target_arch = "wasm32")]
async fn subscribe_realtime_stream<F>(
    after: i64,
    auction_id: Option<&str>,
    mut on_page: F,
) -> Result<(), String>
where
    F: FnMut(RealtimePage),
{
    let expected_stream = auction_id.map_or_else(
        || "auction:discovery".to_owned(),
        |auction_id| format!("auction:{auction_id}"),
    );
    let url = websocket_url(after, auction_id)?;
    let socket = WebSocket::open(&url).map_err(|error| error.to_string())?;
    let (_write, mut read) = socket.split();
    let mut expected_after = after;
    while let Some(message) = read.next().await {
        let message = message.map_err(|error| error.to_string())?;
        let Message::Text(message) = message else {
            return Err("实时 WebSocket 返回了不支持的二进制消息".to_owned());
        };
        let page = decode_realtime_page(&message)?;
        validate_realtime_page(&page, expected_after, &expected_stream)?;
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn subscribe_auction_realtime<F>(
    _after: i64,
    _auction_id: String,
    _on_page: F,
) -> Result<(), String>
where
    F: FnMut(RealtimePage),
{
    Err("实时 WebSocket 仅在 wasm32 浏览器目标可用".to_owned())
}

#[cfg(target_arch = "wasm32")]
fn websocket_url(after: i64, auction_id: Option<&str>) -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "浏览器窗口不可用".to_owned())?;
    let location = window.location();
    let protocol = location.protocol().map_err(|_| "读取浏览器协议失败".to_owned())?;
    let host = location.host().map_err(|_| "读取浏览器主机失败".to_owned())?;
    let websocket_protocol = match protocol.as_str() {
        "http:" => "ws:",
        "https:" => "wss:",
        _ => return Err("浏览器协议不支持 WebSocket".to_owned()),
    };
    let mut query = form_urlencoded::Serializer::new(String::new());
    if let Some(auction_id) = auction_id {
        query.append_pair("auction_id", auction_id);
    } else {
        query.append_pair("scope", "auctions");
    }
    query.append_pair("after", &after.max(0).to_string());
    query.append_pair("page_size", "50");
    Ok(format!("{websocket_protocol}//{host}/api/v1/ws?{}", query.finish()))
}

#[cfg(target_arch = "wasm32")]
fn decode_realtime_page(message: &str) -> Result<RealtimePage, String> {
    let payload: ApiResponse<RealtimePage> =
        serde_json::from_str(message).map_err(|error| error.to_string())?;
    Ok(payload.data)
}

fn validate_realtime_page(
    page: &RealtimePage,
    after: i64,
    expected_stream: &str,
) -> Result<(), String> {
    let mut expected_after = after;
    for event in &page.events {
        if !valid_realtime_event(event) || !event_matches_stream(event, expected_stream) {
            return Err("实时事件负载不符合已冻结契约".to_owned());
        }
        if event.stream_key != expected_stream
            || event.stream_sequence != expected_after.saturating_add(1)
        {
            return Err("实时事件序列出现断档".to_owned());
        }
        expected_after = event.stream_sequence;
    }
    if page.next_after != expected_after {
        return Err("实时游标与事件页不一致".to_owned());
    }
    Ok(())
}

fn event_matches_stream(event: &RealtimeEvent, expected_stream: &str) -> bool {
    let Some(stream_auction_id) = expected_stream.strip_prefix("auction:") else {
        return false;
    };
    if stream_auction_id == "discovery" {
        return true;
    }
    let Some(payload_auction_id) = event.payload.get("auction_id").and_then(Value::as_str) else {
        return false;
    };
    Uuid::parse_str(stream_auction_id).ok() == Uuid::parse_str(payload_auction_id).ok()
}

fn valid_realtime_event(event: &RealtimeEvent) -> bool {
    let Ok(event_id) = Uuid::parse_str(&event.event_id) else {
        return false;
    };
    if event_id.is_nil()
        || event.stream_sequence <= 0
        || DateTime::parse_from_rfc3339(&event.created_at).is_err()
    {
        return false;
    }

    let Some(object) = event.payload.as_object() else {
        return false;
    };
    let fields = match (event.kind.as_str(), event.schema_version) {
        ("auction.created", 1) => &["auction_id", "world_id", "sequence", "state_version"][..],
        ("auction.preview_updated", 1) => {
            &["auction_id", "preview_revision", "item_count", "manual_open", "state_version"][..]
        }
        ("preview_catalog_changed", 1) => &["world", "auction_id", "preview_revision"][..],
        ("auction.opened", 1) => {
            &["auction_id", "world_id", "state_version", "preview_revision"][..]
        }
        ("auction.final_phase_entered", 1) | ("auction.cancelled", 1) | ("auction.settling", 1) => {
            &["auction_id", "state_version"][..]
        }
        ("auction.settled", 1) => {
            &["auction_id", "sold_count", "unsold_count", "state_version"][..]
        }
        ("auction.settled", 2) => &[
            "auction_id",
            "state_version",
            "policy_version",
            "sold_count",
            "unsold_count",
            "obligation_count",
        ][..],
        ("lot.bid_accepted", 2) => &[
            "auction_id",
            "lot_id",
            "bid_id",
            "amount",
            "effective_end",
            "state_version",
            "fact_index",
            "mechanism",
        ][..],
        _ => return false,
    };
    if object.len() != fields.len()
        || fields.iter().any(|field| object.get(*field).is_none_or(Value::is_null))
    {
        return false;
    }

    let uuid_field = |field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .is_some_and(|value| !value.is_nil())
    };
    let text_field = |field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty() && value == value.trim())
    };
    let nonnegative_integer =
        |field: &str| object.get(field).and_then(Value::as_i64).is_some_and(|value| value >= 0);
    let positive_integer =
        |field: &str| object.get(field).and_then(Value::as_i64).is_some_and(|value| value > 0);

    uuid_field("auction_id")
        && match event.kind.as_str() {
            "auction.created" => {
                text_field("world_id")
                    && positive_integer("sequence")
                    && positive_integer("state_version")
            }
            "auction.preview_updated" => {
                nonnegative_integer("preview_revision")
                    && nonnegative_integer("item_count")
                    && object.get("manual_open").is_some_and(Value::is_boolean)
                    && positive_integer("state_version")
            }
            "preview_catalog_changed" => {
                text_field("world") && nonnegative_integer("preview_revision")
            }
            "auction.opened" => {
                text_field("world_id")
                    && positive_integer("state_version")
                    && nonnegative_integer("preview_revision")
            }
            "auction.final_phase_entered" | "auction.cancelled" | "auction.settling" => {
                positive_integer("state_version")
            }
            "auction.settled" => {
                positive_integer("state_version")
                    && nonnegative_integer("sold_count")
                    && nonnegative_integer("unsold_count")
                    && (event.schema_version == 1
                        || (nonnegative_integer("policy_version")
                            && nonnegative_integer("obligation_count")))
            }
            "lot.bid_accepted" => {
                uuid_field("lot_id")
                    && uuid_field("bid_id")
                    && positive_integer("amount")
                    && DateTime::parse_from_rfc3339(
                        object.get("effective_end").and_then(Value::as_str).unwrap_or_default(),
                    )
                    .is_ok()
                    && positive_integer("state_version")
                    && positive_integer("fact_index")
                    && object
                        .get("mechanism")
                        .and_then(Value::as_str)
                        .is_some_and(|value| matches!(value, "fast" | "slow"))
            }
            _ => false,
        }
}
