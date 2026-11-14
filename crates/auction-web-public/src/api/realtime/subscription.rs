//! HTTP catch-up 与 WebSocket 订阅生命周期。
//! 连接建立只受握手超时约束；业务消息空闲不能证明断线，协议 Ping/Pong 不暴露给浏览器 API。
//! 每帧先解码，序列跳号时回到 HTTP 补发，缺口仍存在才降级重连。

#[cfg(target_arch = "wasm32")]
use std::pin::Pin;

#[cfg(target_arch = "wasm32")]
use auction_web_shared::timer::TimeoutFuture;
#[cfg(target_arch = "wasm32")]
use auction_web_shared::{BrowserErrorClass, report_browser_error};
#[cfg(target_arch = "wasm32")]
use futures_util::{
    Sink, StreamExt,
    future::{Either, poll_fn, select},
    pin_mut,
};
#[cfg(target_arch = "wasm32")]
use gloo_net::websocket::{Message, State as WebSocketState, futures::WebSocket};
#[cfg(target_arch = "wasm32")]
use url::form_urlencoded;

#[cfg(not(target_arch = "wasm32"))]
use super::protocol::{RealtimeError, RealtimePage};
#[cfg(target_arch = "wasm32")]
use super::protocol::{
    RealtimeError, RealtimePage, canonical_auction_id, validate_realtime_page_sequence,
    websocket_url,
};
#[cfg(target_arch = "wasm32")]
use crate::api::send_get;

/// 只限制连接建立阶段；业务消息空闲不能证明断线，协议 Ping/Pong 不暴露给浏览器 API。
#[cfg(target_arch = "wasm32")]
const REALTIME_CONNECT_TIMEOUT_MILLIS: u32 = 10_000;
/// 连续毒帧上限：单帧解码失败只计数降级，连续 5 帧失败才断开整条订阅。
#[cfg(target_arch = "wasm32")]
const MAX_CONSECUTIVE_BAD_FRAMES: u32 = 5;
/// HTTP catch-up 翻页上限：落后超过 50 页（5000 事件）说明逐页重放代价过大，
/// 转为全量 refetch 降级，由各 resource 重读权威快照。
#[cfg(target_arch = "wasm32")]
const MAX_CATCH_UP_PAGES: u32 = 50;

#[cfg(target_arch = "wasm32")]
async fn fetch_realtime_page(
    after: i64,
    auction_id: Option<&str>,
) -> Result<RealtimePage, RealtimeError> {
    if after < 0 {
        return Err(RealtimeError::permanent("实时事件游标无效"));
    }
    let canonical_auction_id =
        auction_id.map(canonical_auction_id).transpose().map_err(RealtimeError::permanent)?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    if let Some(auction_id) = canonical_auction_id.as_deref() {
        query.append_pair("auction_id", auction_id);
    } else {
        query.append_pair("scope", "auctions");
    }
    query.append_pair("after", &after.to_string()).append_pair("page_size", "100");
    let url = format!("/api/v1/events?{}", query.finish());
    let response = send_get(&url).await.map_err(|error| RealtimeError::transient(error.code()))?;
    if !response.ok() {
        let status = response.status();
        // 401/403 认证或权限失败、404 目标拍卖不存在：重连不能自愈，按永久错误处理。
        return Err(if matches!(status, 401 | 403 | 404) {
            RealtimeError::permanent(format!("HTTP {status}"))
        } else {
            RealtimeError::transient(format!("HTTP {status}"))
        });
    }
    let page = if let Some(auction_id) = canonical_auction_id.as_deref() {
        let payload = crate::api::decoder::decode_auction_realtime_page(response, auction_id)
            .await
            .map_err(RealtimeError::transient)?;
        RealtimePage {
            events: payload.events,
            next_after: payload.next_after,
            has_more: payload.has_more,
            refetch_required: payload.refetch_required,
        }
    } else {
        let payload = crate::api::decoder::decode_discovery_realtime_page(response)
            .await
            .map_err(RealtimeError::transient)?;
        RealtimePage {
            events: payload.events,
            next_after: payload.next_after,
            has_more: payload.has_more,
            refetch_required: payload.refetch_required,
        }
    };
    validate_realtime_page_sequence(&page, after).map_err(RealtimeError::transient)?;
    Ok(page)
}

/// 订阅 Public 持久化实时流，并把服务端事件页交给页面替换当前安全快照。
/// 浏览器 WebSocket 自动携带同源 HttpOnly session Cookie；匿名客户端由服务端拒绝。
/// 连接建立后先走 HTTP catch-up，期间到达的单事件帧由 WebSocket 缓冲，避免补发与订阅间隙。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn subscribe_realtime<F>(after: i64, on_page: F) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    subscribe_realtime_stream(after, None, None, on_page).await
}

/// 订阅单个拍卖的实时流；拍卖页面用它对齐旧版的 lot/竞价即时刷新边界。
#[cfg(target_arch = "wasm32")]
pub(crate) async fn subscribe_auction_realtime<F>(
    after: i64,
    auction_id: String,
    world: String,
    on_page: F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    subscribe_realtime_stream(after, Some(&auction_id), Some(&world), on_page).await
}

#[cfg(target_arch = "wasm32")]
async fn subscribe_realtime_stream<F>(
    after: i64,
    auction_id: Option<&str>,
    world: Option<&str>,
    mut on_page: F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    if after < 0 {
        return Err(RealtimeError::permanent("实时事件游标无效"));
    }
    let canonical_auction_id =
        auction_id.map(canonical_auction_id).transpose().map_err(RealtimeError::permanent)?;
    let auction_id = canonical_auction_id.as_deref();
    let url = websocket_url(auction_id).map_err(RealtimeError::permanent)?;
    let mut socket =
        WebSocket::open(&url).map_err(|error| RealtimeError::transient(error.to_string()))?;
    {
        let connected =
            poll_fn(|context| Sink::<Message>::poll_ready(Pin::new(&mut socket), context));
        let deadline = TimeoutFuture::new(REALTIME_CONNECT_TIMEOUT_MILLIS);
        pin_mut!(connected, deadline);
        match select(connected, deadline).await {
            Either::Left((result, _)) => {
                result.map_err(|error| RealtimeError::transient(error.to_string()))?;
            }
            Either::Right(((), _)) => {
                return Err(RealtimeError::transient("实时 WebSocket 握手超时"));
            }
        }
    }
    if !matches!(socket.state(), WebSocketState::Open) {
        return Err(RealtimeError::transient("实时 WebSocket 建立失败"));
    }
    let (_write, mut read) = socket.split();
    let mut expected_after = after;
    catch_up_realtime(auction_id, &mut expected_after, &mut on_page).await?;
    let mut consecutive_bad_frames = 0_u32;
    // 存活检查由服务端 Ping/Pong 拥有；静默拍卖保持连接，关闭/错误才触发恢复。
    while let Some(message) = read.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                return Err(RealtimeError::transient(format!("实时 WebSocket 读取失败：{error}")));
            }
        };
        let Message::Text(message) = message else {
            // 二进制帧不直接终止订阅：按毒帧计数，连续超限才断开重连。
            report_browser_error(BrowserErrorClass::RealtimeProtocol);
            consecutive_bad_frames += 1;
            if consecutive_bad_frames >= MAX_CONSECUTIVE_BAD_FRAMES {
                return Err(RealtimeError::transient("实时 WebSocket 连续返回不支持的二进制帧"));
            }
            continue;
        };
        // 建连首帧报告服务端构建号；识别后只更新偏差提示，不进入事件序列。
        if let Some(server_build) = crate::api::decoder::decode_build_hello(&message) {
            auction_web_shared::observe_server_build_id(&server_build);
            continue;
        }
        let event = match crate::api::decoder::decode_realtime_event(&message, auction_id, world) {
            Ok(event) => event,
            Err(error) => {
                // 单帧解码失败只计毒帧，不终止整条订阅；连续超限才断开重连。
                report_browser_error(BrowserErrorClass::RealtimeProtocol);
                consecutive_bad_frames += 1;
                if consecutive_bad_frames >= MAX_CONSECUTIVE_BAD_FRAMES {
                    return Err(RealtimeError::transient(format!(
                        "实时事件帧连续解码失败：{error}"
                    )));
                }
                continue;
            }
        };
        consecutive_bad_frames = 0;
        if event.stream_sequence <= expected_after {
            continue;
        }
        let Some(next_sequence) = expected_after.checked_add(1) else {
            return Err(RealtimeError::permanent("实时事件序列超过可表示范围"));
        };
        if event.stream_sequence != next_sequence {
            catch_up_realtime(auction_id, &mut expected_after, &mut on_page).await?;
        }
        if event.stream_sequence <= expected_after {
            continue;
        }
        let Some(next_sequence) = expected_after.checked_add(1) else {
            return Err(RealtimeError::permanent("实时事件序列超过可表示范围"));
        };
        if event.stream_sequence != next_sequence {
            report_browser_error(BrowserErrorClass::RealtimeProtocol);
            return Err(RealtimeError::transient("实时事件序列出现断档"));
        }
        expected_after = event.stream_sequence;
        let page = RealtimePage {
            events: vec![event],
            next_after: expected_after,
            has_more: false,
            refetch_required: false,
        };
        let _ = on_page(page);
    }
    Err(RealtimeError::transient("实时 WebSocket 已关闭"))
}

#[cfg(target_arch = "wasm32")]
async fn catch_up_realtime<F>(
    auction_id: Option<&str>,
    expected_after: &mut i64,
    on_page: &mut F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    let mut fetched_pages = 0_u32;
    loop {
        let requested_after = *expected_after;
        let page = fetch_realtime_page(requested_after, auction_id).await?;
        let next_after = page.next_after;
        let has_more = page.has_more;
        let refetch_required = page.refetch_required;
        if has_more && next_after == requested_after {
            return Err(RealtimeError::transient("实时事件补发游标未前进"));
        }
        *expected_after = next_after;
        let continue_catch_up = on_page(page);
        if refetch_required || !continue_catch_up {
            return Ok(());
        }
        if !has_more {
            return Ok(());
        }
        fetched_pages += 1;
        if fetched_pages >= MAX_CATCH_UP_PAGES {
            // 落后页数超过上限：放弃逐页补齐，合成 refetch 页让各 resource 全量重读权威快照。
            tracing::warn!(
                auction_id = ?auction_id,
                after = *expected_after,
                fetched_pages,
                "实时 catch-up 翻页超限，降级为全量 refetch"
            );
            let degraded_page = RealtimePage {
                events: Vec::new(),
                next_after: *expected_after,
                has_more: false,
                refetch_required: true,
            };
            let _ = on_page(degraded_page);
            return Ok(());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn subscribe_realtime<F>(_after: i64, _on_page: F) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    Err(RealtimeError::permanent("实时 WebSocket 仅在 wasm32 浏览器目标可用"))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn subscribe_auction_realtime<F>(
    _after: i64,
    _auction_id: String,
    _world: String,
    _on_page: F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    Err(RealtimeError::permanent("实时 WebSocket 仅在 wasm32 浏览器目标可用"))
}
