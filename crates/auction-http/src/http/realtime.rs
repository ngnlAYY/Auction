//! Public 持久化实时事件回放 HTTP 适配器。
//!
//! 该模块只解析冻结的查询参数并映射安全事件 DTO；WebSocket 只做有界的服务端推送，不能
//! 绕过本回放接口，也不能把浏览器消息当作业务命令。

use auction_application::{
    MAX_PUBLIC_EVENT_PAGE_SIZE, PublicRealtimeEvent, PublicRealtimePage, PublicRealtimeRequest,
};
use auction_contracts::{
    ApiErrorResponse, ApiResponse, PublicRealtimeEventResponse, PublicRealtimeResponse,
};
use axum::{
    Json,
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::net::SocketAddr;
use tokio::time::{Duration, interval};
use uuid::Uuid;

use super::{
    PublicApiState, api_error, not_found_error, parse_positive_i64, parse_query, timestamp,
};

/// 读取指定拍卖流或公开发现流的持久化事件。
pub(crate) async fn list(
    State(state): State<PublicApiState>,
    uri: axum::http::Uri,
) -> Result<Json<ApiResponse<PublicRealtimeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let query = parse_query(&uri, &["auction_id", "after", "page_size"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let auction_id = query
        .get("auction_id")
        .map(|value| {
            Uuid::parse_str(value).map_err(|_| {
                api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            })
        })
        .transpose()?;
    let after =
        query
            .get("after")
            .map(|value| {
                value.parse::<i64>().ok().filter(|value| *value >= 0).ok_or_else(|| {
                    api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                })
            })
            .transpose()?
            .unwrap_or(0);
    let page_size =
        parse_positive_i64(query.get("page_size"), 50, Some(MAX_PUBLIC_EVENT_PAGE_SIZE))
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let request = PublicRealtimeRequest { auction_id, after, page_size };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

    let Some(reader) = state.realtime() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "实时事件暂不可用",
        ));
    };
    let page = reader.list_public_events(request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "实时事件暂不可用")
    })?;
    let Some(page) = page else {
        return Err(not_found_error());
    };
    if !valid_page(&page, auction_id, after) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "实时事件暂不可用",
        ));
    }

    Ok(Json(ApiResponse::ok("实时事件已加载", map_page(page))))
}

/// 建立只读 WebSocket；连接范围必须明确为单一拍卖流或公开发现流。
pub(crate) async fn websocket(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Some(response) = super::session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Err(response) = super::authenticated_public_identity(&state, &headers).await {
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
    let page_size =
        match parse_positive_i64(query.get("page_size"), 50, Some(MAX_PUBLIC_EVENT_PAGE_SIZE)) {
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

    upgrade
        .on_upgrade(move |socket| stream_events(socket, state, request, connection_permit))
        .into_response()
}

async fn stream_events(
    mut socket: WebSocket,
    state: PublicApiState,
    mut request: PublicRealtimeRequest,
    _connection_permit: tokio::sync::OwnedSemaphorePermit,
) {
    let Some(reader) = state.realtime() else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    let mut ticker = interval(Duration::from_millis(500));
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
            _ = ticker.tick() => {
                let page = match reader.list_public_events(request.clone()).await {
                    Ok(Some(page)) => page,
                    Ok(None) | Err(_) => {
                        let _ = socket.send(Message::Close(None)).await;
                        return;
                    }
                };
                if !valid_page(&page, request.auction_id, request.after) {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
                let refetch_required = page.refetch_required;
                let next_after = page.next_after;
                if page.events.is_empty() && !refetch_required {
                    if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                        return;
                    }
                    continue;
                }
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
                if refetch_required {
                    let _ = socket.send(Message::Close(None)).await;
                    return;
                }
                request.after = next_after;
            }
        }
    }
}

fn valid_page(page: &PublicRealtimePage, auction_id: Option<Uuid>, after: i64) -> bool {
    let expected_stream = auction_id.map_or_else(
        || "auction:discovery".to_owned(),
        |auction_id| format!("auction:{auction_id}"),
    );
    let mut previous = after;
    for event in &page.events {
        if event.validate().is_err()
            || event.stream_key != expected_stream
            || event.stream_sequence <= previous
        {
            return false;
        }
        previous = event.stream_sequence;
    }
    page.next_after == previous
}

fn map_page(page: PublicRealtimePage) -> PublicRealtimeResponse {
    PublicRealtimeResponse {
        events: page.events.into_iter().map(map_event).collect(),
        next_after: page.next_after,
        has_more: page.has_more,
        refetch_required: page.refetch_required,
    }
}

fn map_event(event: PublicRealtimeEvent) -> PublicRealtimeEventResponse {
    PublicRealtimeEventResponse {
        event_id: event.event_id.to_string(),
        stream_key: event.stream_key,
        stream_sequence: event.stream_sequence,
        kind: event.kind,
        schema_version: event.schema_version,
        payload: event.payload,
        created_at: timestamp(event.created_at),
    }
}
