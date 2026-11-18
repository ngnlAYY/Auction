//! HTTP catch-up 与 WebSocket 订阅生命周期。
//! 连接建立只受握手超时约束；业务消息空闲不能证明断线，协议 Ping/Pong 不暴露给浏览器 API。
//! 每帧先解码，序列跳号时回到 HTTP 补发，缺口仍存在才降级重连。

#[cfg(target_arch = "wasm32")]
use std::pin::Pin;

#[cfg(target_arch = "wasm32")]
use auction_web_shared::timer::TimeoutFuture;
#[cfg(target_arch = "wasm32")]
use auction_web_shared::{BrowserErrorClass, report_browser_error};
#[cfg(any(target_arch = "wasm32", test))]
use futures_util::{FutureExt, Stream, StreamExt};
#[cfg(target_arch = "wasm32")]
use futures_util::{
    Sink,
    future::{Either, poll_fn, select},
    pin_mut,
};
#[cfg(target_arch = "wasm32")]
use gloo_net::websocket::futures::WebSocket;
#[cfg(any(target_arch = "wasm32", test))]
use gloo_net::websocket::{Message, State as WebSocketState, WebSocketError};
#[cfg(target_arch = "wasm32")]
use url::form_urlencoded;

#[cfg(not(target_arch = "wasm32"))]
use super::protocol::{RealtimeError, RealtimePage};
#[cfg(target_arch = "wasm32")]
use super::protocol::{
    RealtimeError, RealtimePage, canonical_auction_id, validate_realtime_page_sequence,
    websocket_url,
};
#[cfg(any(target_arch = "wasm32", test))]
use crate::api::TransportError;
#[cfg(target_arch = "wasm32")]
use crate::api::clock::observe_response_date;

/// 握手超时；只限制连接建立阶段（见模块文档）。
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
async fn fetch_realtime_page(after: i64, auction_id: &str) -> Result<RealtimePage, RealtimeError> {
    if after < 0 {
        return Err(RealtimeError::permanent("实时事件游标无效"));
    }
    let auction_id = canonical_auction_id(auction_id).map_err(RealtimeError::permanent)?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("auction_id", &auction_id)
        .append_pair("after", &after.to_string())
        .append_pair("page_size", "100");
    let url = format!("/api/v1/events?{}", query.finish());
    let response = read_catch_up_response(async |status| {
        auction_web_shared::send_get_observed(&url, |response| {
            *status = Some(response.status());
            observe_response_date(response);
        })
        .await
    })
    .await?;
    if !response.ok() {
        return Err(RealtimeError::transient(format!("HTTP {}", response.status())));
    }
    let payload = crate::api::decoder::decode_auction_realtime_page(response, &auction_id)
        .await
        .map_err(RealtimeError::transient)?;
    let page = RealtimePage {
        events: payload.events,
        next_after: payload.next_after,
        has_more: payload.has_more,
        refetch_required: payload.refetch_required,
        from_websocket: false,
    };
    validate_realtime_page_sequence(&page, after).map_err(RealtimeError::transient)?;
    Ok(page)
}

#[cfg(any(target_arch = "wasm32", test))]
async fn read_catch_up_response<T>(
    send: impl AsyncFnOnce(&mut Option<u16>) -> Result<T, TransportError>,
) -> Result<T, RealtimeError> {
    let mut status = None;
    let result = send(&mut status).await;
    // 认证、权限或目标不存在已由响应头确认，正文断流或超时不能重开重连。
    if let Some(status @ (401 | 403 | 404)) = status {
        return Err(RealtimeError::permanent(format!("HTTP {status}")));
    }
    result.map_err(|error| RealtimeError::transient(error.code()))
}

/// 订阅单个拍卖的实时流，并把服务端事件页交给页面替换当前安全快照；拍卖页面用它对齐旧版的
/// lot/竞价即时刷新边界。浏览器 WebSocket 自动携带同源 HttpOnly session Cookie，匿名客户端由
/// 服务端拒绝。连接建立后先走 HTTP catch-up，期间到达的单事件帧由 WebSocket 缓冲，避免补发与
/// 订阅间隙。
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
    if after < 0 {
        return Err(RealtimeError::permanent("实时事件游标无效"));
    }
    let auction_id = canonical_auction_id(&auction_id).map_err(RealtimeError::permanent)?;
    let url = websocket_url(&auction_id).map_err(RealtimeError::permanent)?;
    let mut socket =
        WebSocket::open(&url).map_err(|error| RealtimeError::transient(error.to_string()))?;
    run_subscription(&mut socket, async |socket| {
        listen_realtime(socket, after, &auction_id, &world, on_page).await
    })
    .await
}

#[cfg(target_arch = "wasm32")]
async fn listen_realtime<F>(
    socket: &mut WebSocket,
    after: i64,
    auction_id: &str,
    world: &str,
    mut on_page: F,
) -> Result<(), RealtimeError>
where
    F: FnMut(RealtimePage) -> bool,
{
    {
        let connected =
            poll_fn(|context| Sink::<Message>::poll_ready(Pin::new(&mut *socket), context));
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
    check_websocket_open(socket.state())?;
    let mut expected_after = after;
    catch_up_realtime(auction_id, &mut expected_after, &mut on_page).await?;
    let mut consecutive_bad_frames = 0_u32;
    // 存活检查由服务端 Ping/Pong 拥有；静默拍卖保持连接，关闭/错误才触发恢复。
    while let Some(message) = socket.next().await {
        let message = message.map_err(websocket_read_error)?;
        let Message::Text(message) = message else {
            // 二进制帧同样按毒帧计数，不直接终止订阅。
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
        let decoded = crate::api::decoder::decode_realtime_event(&message, auction_id, world);
        let event = match decoded {
            Ok(event) => event,
            Err(error) => {
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
            from_websocket: true,
        };
        let _ = on_page(page);
    }
    Err(RealtimeError::transient("实时 WebSocket 已关闭"))
}

#[cfg(any(target_arch = "wasm32", test))]
fn check_websocket_open(state: WebSocketState) -> Result<(), RealtimeError> {
    if matches!(state, WebSocketState::Open) {
        Ok(())
    } else {
        Err(RealtimeError::transient("实时 WebSocket 建立失败"))
    }
}

#[cfg(any(target_arch = "wasm32", test))]
async fn run_subscription<S>(
    socket: &mut S,
    listen: impl AsyncFnOnce(&mut S) -> Result<(), RealtimeError>,
) -> Result<(), RealtimeError>
where
    S: Stream<Item = Result<Message, WebSocketError>> + Unpin,
{
    listen(socket).await.map_err(|error| with_queued_close_code(error, socket))
}

#[cfg(any(target_arch = "wasm32", test))]
fn with_queued_close_code(
    error: RealtimeError,
    read: &mut (impl Stream<Item = Result<Message, WebSocketError>> + Unpin),
) -> RealtimeError {
    if error.close_code().is_some() {
        return error;
    }
    // 每次连接结束只交付一个错误。退出时仅检查已排队的帧，普通事件可由下次回放补齐。
    while let Some(Some(message)) = read.next().now_or_never() {
        if let Err(WebSocketError::ConnectionClose(event)) = message {
            return error.with_close_code(event.code);
        }
    }
    error
}

#[cfg(any(target_arch = "wasm32", test))]
fn websocket_read_error(error: WebSocketError) -> RealtimeError {
    let detail = format!("实时 WebSocket 读取失败：{error}");
    match error {
        WebSocketError::ConnectionClose(event) => {
            RealtimeError::transient(detail).with_close_code(event.code)
        }
        _ => RealtimeError::transient(detail),
    }
}

#[cfg(target_arch = "wasm32")]
async fn catch_up_realtime<F>(
    auction_id: &str,
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
                auction_id,
                after = *expected_after,
                fetched_pages,
                "实时 catch-up 翻页超限，降级为全量 refetch"
            );
            let degraded_page = RealtimePage {
                events: Vec::new(),
                next_after: *expected_after,
                has_more: false,
                refetch_required: true,
                from_websocket: false,
            };
            let _ = on_page(degraded_page);
            return Ok(());
        }
    }
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
    #[cfg(test)]
    if let Some(fixture) =
        crate::surface::PageReadFixture::<(i64, String, String), ReplayFixture>::response((
            _after,
            _auction_id,
            _world,
        ))
    {
        return read_catch_up_response(async |status| {
            *status = Some(fixture.status);
            fixture.body.await
        })
        .await
        .map_err(|error| error.with_close_code(fixture.close_code));
    }
    Err(RealtimeError::permanent("实时 WebSocket 仅在 wasm32 浏览器目标可用"))
}

#[cfg(test)]
pub(crate) struct ReplayFixture {
    pub(crate) status: u16,
    pub(crate) close_code: u16,
    pub(crate) body:
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>>>>,
}

#[cfg(test)]
mod tests {
    use futures_util::{FutureExt, StreamExt, stream};
    use gloo_net::websocket::{
        Message, State as WebSocketState, WebSocketError, events::CloseEvent,
    };

    use super::super::protocol::RealtimeError;
    use super::{
        check_websocket_open, read_catch_up_response, run_subscription, websocket_read_error,
    };
    use crate::api::TransportError;

    #[test]
    fn interrupted_catch_up_bodies_keep_the_http_reconnect_policy() {
        for status in [401, 403, 404, 200, 503] {
            for transport in [TransportError::Timeout, TransportError::Network(Some(status))] {
                let error = read_catch_up_response::<()>(async |head| {
                    *head = Some(status);
                    Err(transport)
                })
                .now_or_never()
                .unwrap()
                .unwrap_err();
                assert_eq!(error.is_permanent(), matches!(status, 401 | 403 | 404));
                if error.is_permanent() {
                    assert_eq!(error.detail(), format!("HTTP {status}"));
                } else {
                    assert_eq!(error.detail(), transport.code());
                }
            }
        }
        assert!(
            read_catch_up_response(async |head| {
                *head = Some(200);
                Ok(())
            })
            .now_or_never()
            .unwrap()
            .is_ok()
        );
    }

    #[test]
    fn websocket_close_keeps_the_code_and_the_transient_retry_policy() {
        for code in [1008, 1013, 1001] {
            let error = websocket_read_error(WebSocketError::ConnectionClose(CloseEvent {
                code,
                reason: String::new(),
                was_clean: true,
            }));
            assert_eq!(error.close_code(), Some(code));
            assert!(!error.is_permanent());
        }
        let network_error = websocket_read_error(WebSocketError::ConnectionError);
        assert_eq!(network_error.close_code(), None);
        assert!(!network_error.is_permanent());
    }

    #[test]
    fn failed_catch_up_preserves_already_buffered_close_codes() {
        for code in [1008, 1013] {
            let mut read = stream::iter([
                Ok(Message::Text("buffered event".to_owned())),
                Err(WebSocketError::ConnectionError),
                Err(WebSocketError::ConnectionClose(CloseEvent {
                    code,
                    reason: String::new(),
                    was_clean: true,
                })),
            ])
            .chain(stream::pending());
            let error =
                run_subscription(&mut read, async |_| Err(RealtimeError::permanent("HTTP 404")))
                    .now_or_never()
                    .unwrap()
                    .unwrap_err();
            assert_eq!(error.close_code(), Some(code));
            assert!(error.is_permanent());
            assert_eq!(error.detail(), "HTTP 404");
        }
    }

    #[test]
    fn failed_catch_up_without_a_buffered_close_keeps_its_http_error() {
        let mut read =
            stream::iter([Ok(Message::Text("buffered event".to_owned()))]).chain(stream::pending());
        let http_error = RealtimeError::transient("HTTP 503");
        let error = run_subscription(&mut read, async |_| Err(http_error.clone()))
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert_eq!(error, http_error);
    }

    #[test]
    fn connection_error_exit_recovers_the_queued_close_code() {
        let close = CloseEvent { code: 1008, reason: String::new(), was_clean: true };
        let mut read = stream::iter([
            Err(WebSocketError::ConnectionError),
            Ok(Message::Text("buffered event".to_owned())),
            Err(WebSocketError::ConnectionClose(close)),
        ]);
        let error = run_subscription(&mut read, async |socket| {
            socket.next().await.unwrap().map(drop).map_err(websocket_read_error)
        })
        .now_or_never()
        .unwrap()
        .unwrap_err();
        let connection_error = websocket_read_error(WebSocketError::ConnectionError);
        assert_eq!(error.close_code(), Some(1008));
        assert_eq!(error.is_permanent(), connection_error.is_permanent());
        assert_eq!(error.detail(), connection_error.detail());
    }

    #[test]
    fn socket_closed_after_handshake_recovers_the_queued_close_code() {
        let close = CloseEvent { code: 1008, reason: String::new(), was_clean: true };
        let mut read = stream::iter([Err(WebSocketError::ConnectionClose(close))]);
        let error =
            run_subscription(&mut read, async |_| check_websocket_open(WebSocketState::Closed))
                .now_or_never()
                .unwrap()
                .unwrap_err();
        assert_eq!(error.close_code(), Some(1008));
        assert!(!error.is_permanent());
        assert_eq!(error.detail(), "实时 WebSocket 建立失败");
    }
}
