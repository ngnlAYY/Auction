//! Public 实时 WebSocket 建立、补发和连接生命周期。

use std::net::SocketAddr;

use auction_application::{PublicRealtimePage, PublicRealtimeReader, PublicRealtimeRequest};
use axum::{
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use tokio::time::{Duration, interval};
use uuid::Uuid;

use super::super::realtime_hub::RealtimeSubscription;
use super::super::{PublicApiState, api_error, not_found_error, parse_positive_i64, parse_query};
use super::mapping::{map_page, valid_page};

/// 建立只读 WebSocket；连接范围必须明确为单一拍卖流或公开发现流。
pub(crate) async fn websocket(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Some(response) = super::super::session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Err(response) = super::super::authenticated_public_identity(&state, &headers).await {
        return *response;
    }
    let query = match parse_query(&uri, &["auction_id", "scope", "after", "page_size"]) {
        Ok(query) => query,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let auction_id = match query.get("auction_id") {
        Some(value) => match Uuid::parse_str(value) {
            Ok(value) if !value.is_nil() => Some(value),
            _ => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
                    .into_response();
            }
        },
        None => None,
    };
    let scope = query.get("scope").map(String::as_str);
    if (auction_id.is_some() && scope.is_some())
        || (auction_id.is_none() && scope != Some("auctions"))
    {
        return api_error(StatusCode::BAD_REQUEST, "invalid_scope", "WebSocket 范围无效")
            .into_response();
    }
    let after = match query.get("after") {
        Some(value) => match value.parse::<i64>() {
            Ok(value) if value >= 0 => value,
            _ => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                    .into_response();
            }
        },
        None => 0,
    };
    let page_size = match parse_positive_i64(
        query.get("page_size"),
        50,
        Some(auction_application::MAX_PUBLIC_EVENT_PAGE_SIZE),
    ) {
        Ok(value) => value,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request = PublicRealtimeRequest { auction_id, after, page_size };
    if request.validate().is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    }
    let Some(reader) = state.realtime() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "实时事件暂不可用",
        )
        .into_response();
    };
    match reader.list_public_events(request.clone()).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found_error().into_response(),
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "实时事件暂不可用",
            )
            .into_response();
        }
    }
    let Some(connection_permit) = state.try_acquire_realtime_connection() else {
        return api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "realtime_capacity_exhausted",
            "实时连接资源已耗尽",
        )
        .into_response();
    };
    let Some(hub) = state.realtime_hub() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "realtime_unavailable",
            "实时连接暂不可用",
        )
        .into_response();
    };
    let stream_key = request.auction_id.map_or_else(
        || "auction:discovery".to_owned(),
        |auction_id| format!("auction:{auction_id}"),
    );
    let subscription = match hub.subscribe(stream_key) {
        Ok(subscription) => subscription,
        Err(_) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "realtime_unavailable",
                "实时连接暂不可用",
            )
            .into_response();
        }
    };

    upgrade
        .on_upgrade(move |socket| {
            stream_events(socket, state, request, subscription, connection_permit)
        })
        .into_response()
}

async fn stream_events(
    mut socket: WebSocket,
    state: PublicApiState,
    mut request: PublicRealtimeRequest,
    mut subscription: RealtimeSubscription,
    _connection_permit: tokio::sync::OwnedSemaphorePermit,
) {
    let Some(reader) = state.realtime() else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    if !send_catch_up(&mut socket, reader, &mut request).await {
        return;
    }
    let mut ticker = interval(Duration::from_secs(25));
    ticker.tick().await;
    loop {
        tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => {
                        let _ = socket.send(Message::Close(None)).await;
                        return;
                    }
                    Some(Err(_)) => return,
                }
            }
            event = subscription.recv() => {
                let Some(event) = event else {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                };
                let expected_stream = request.auction_id.map_or_else(
                    || "auction:discovery".to_owned(),
                    |auction_id| format!("auction:{auction_id}"),
                );
                if event.validate().is_err() || event.stream_key != expected_stream {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
                if event.stream_sequence <= request.after {
                    continue;
                }
                let Some(expected_sequence) = request.after.checked_add(1) else {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                };
                if event.stream_sequence != expected_sequence
                    && !send_catch_up(&mut socket, reader, &mut request).await
                {
                    return;
                }
                if event.stream_sequence <= request.after {
                    continue;
                }
                let Some(expected_sequence) = request.after.checked_add(1) else {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                };
                if event.stream_sequence != expected_sequence {
                    // 当前事件前仍有未送达事件；让客户端通过下一次 reconnect/catch-up 恢复，
                    // 不在 WebSocket 中伪造连续序号。
                    continue;
                }
                let next_after = event.stream_sequence;
                let page = PublicRealtimePage {
                    events: vec![event],
                    next_after,
                    has_more: false,
                    refetch_required: false,
                };
                let message = match serde_json::to_string(&map_page(page)) {
                    Ok(message) => message,
                    Err(_) => {
                        let _ = socket.send(Message::Close(None)).await;
                        return;
                    }
                };
                if socket.send(Message::Text(message.into())).await.is_err() {
                    return;
                }
                request.after = next_after;
            }
            _ = ticker.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn send_catch_up(
    socket: &mut WebSocket,
    reader: &dyn PublicRealtimeReader,
    request: &mut PublicRealtimeRequest,
) -> bool {
    let page = match reader.list_public_events(request.clone()).await {
        Ok(Some(page)) => page,
        Ok(None) | Err(_) => {
            let _ = socket.send(Message::Close(None)).await;
            return false;
        }
    };
    if !valid_page(&page, request.auction_id, request.after) {
        let _ = socket.send(Message::Close(None)).await;
        return false;
    }
    let refetch_required = page.refetch_required;
    let next_after = page.next_after;
    if !page.events.is_empty() || refetch_required {
        let message = match serde_json::to_string(&map_page(page)) {
            Ok(message) => message,
            Err(_) => {
                let _ = socket.send(Message::Close(None)).await;
                return false;
            }
        };
        if socket.send(Message::Text(message.into())).await.is_err() {
            return false;
        }
    }
    request.after = next_after;
    !refetch_required
}
