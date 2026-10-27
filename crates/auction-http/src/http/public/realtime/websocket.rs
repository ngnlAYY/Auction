//! Public 实时 WebSocket 建立、补发和连接生命周期。

use std::net::SocketAddr;

use auction_application::{PublicRealtimeAuctionStatus, PublicRealtimeEvent};
use axum::{
    extract::{
        ConnectInfo, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use tokio::{
    sync::{OwnedSemaphorePermit, watch},
    time::{Duration, Instant, MissedTickBehavior, interval_at, sleep, timeout},
};

use super::super::realtime_hub::{RealtimeSubscription, RealtimeSubscriptionError};
use super::super::{PublicApiState, api_error, not_found_error, parse_query};
use super::mapping::map_event;
use super::session::{RealtimeSession, SessionCheckError};
use super::target::{RealtimeTarget, parse_websocket_target};

const MAX_MESSAGE_BYTES: usize = 4 << 10;
const WRITE_WAIT: Duration = Duration::from_secs(5);
const PONG_WAIT: Duration = Duration::from_secs(60);
const PING_PERIOD: Duration = Duration::from_secs(50);

const CLOSE_GOING_AWAY: u16 = 1001;
const CLOSE_PROTOCOL_ERROR: u16 = 1002;
const CLOSE_POLICY_VIOLATION: u16 = 1008;
const CLOSE_MESSAGE_TOO_BIG: u16 = 1009;
const CLOSE_TRY_AGAIN_LATER: u16 = 1013;

const REASON_SHUTDOWN: &str = "服务正在关闭";
const REASON_PROTOCOL_ERROR: &str = "实时事件合同错误";
const REASON_CLIENT_DATA: &str = "实时连接只允许服务端推送";
const REASON_SLOW_CONSUMER: &str = "连接消费速度过慢";
const REASON_SESSION_EXPIRED: &str = "会话已失效";
const REASON_SESSION_UNAVAILABLE: &str = "会话校验暂不可用";

/// 建立只读 WebSocket；连接范围必须明确为单一拍卖流或公开发现流。
pub(crate) async fn websocket(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: WebSocketUpgrade,
) -> Response {
    // 连接尝试限流是新版保留的附加安全门禁；通过后恢复旧版 origin→query→session 顺序。
    if let Some(response) = super::super::realtime_connection_guard(&state, &headers, address) {
        return response;
    }
    let query = match parse_query(&uri, &["auction_id", "scope"]) {
        Ok(query) => query,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let target = match parse_websocket_target(&query) {
        Ok(target) => target,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let identity = match super::super::authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let (Some(session_service), Some(raw_token)) =
        (state.session_handle(), super::super::read_public_session_cookie(&headers))
    else {
        return super::super::session_unavailable();
    };

    if let RealtimeTarget::Auction(auction_id) = target {
        let Some(reader) = state.auctions() else {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "实时事件暂不可用",
            )
            .into_response();
        };
        match reader.public_realtime_auction_status(auction_id).await {
            Ok(Some(PublicRealtimeAuctionStatus::Settled)) | Ok(None) => {
                return not_found_error().into_response();
            }
            Ok(Some(_)) => {}
            Err(_) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "实时事件暂不可用",
                )
                .into_response();
            }
        }
    }

    let discovery_permit = if matches!(target, RealtimeTarget::Discovery) {
        let Some(permit) = state.try_acquire_realtime_discovery() else {
            return api_error(
                StatusCode::TOO_MANY_REQUESTS,
                "discovery_busy",
                "拍卖发现连接数已达上限",
            )
            .into_response();
        };
        Some(permit)
    } else {
        None
    };
    let Some(hub) = state.realtime_hub() else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "realtime_unavailable",
            "实时连接暂不可用",
        )
        .into_response();
    };
    let subscription = match hub.subscribe(target.stream_key(), identity.user_id) {
        Ok(subscription) => subscription,
        Err(RealtimeSubscriptionError::Capacity) => {
            return api_error(StatusCode::TOO_MANY_REQUESTS, "realtime_busy", "实时连接数已达上限")
                .into_response();
        }
        Err(RealtimeSubscriptionError::Unavailable) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "realtime_unavailable",
                "实时连接暂不可用",
            )
            .into_response();
        }
    };
    let realtime_shutdown = state.realtime_shutdown();
    let session = RealtimeSession::new(session_service, raw_token, identity);

    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| {
            stream_events(
                socket,
                target,
                subscription,
                session,
                discovery_permit,
                realtime_shutdown,
            )
        })
        .into_response()
}

async fn stream_events(
    mut socket: WebSocket,
    target: RealtimeTarget,
    mut subscription: RealtimeSubscription,
    mut session: RealtimeSession,
    _discovery_permit: Option<OwnedSemaphorePermit>,
    mut realtime_shutdown: watch::Receiver<bool>,
) {
    let expected_stream = target.stream_key();
    let mut ticker = interval_at(Instant::now() + PING_PERIOD, PING_PERIOD);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let pong_deadline = sleep(PONG_WAIT);
    tokio::pin!(pong_deadline);

    if *realtime_shutdown.borrow() {
        close_connection(&mut socket, CLOSE_GOING_AWAY, REASON_SHUTDOWN).await;
        return;
    }
    // 升级等待期间可能已注销；必须先完成复核，不能依赖零时长计时器抢在首帧之前。
    if !recheck_session(&mut socket, &mut session).await {
        return;
    }
    let session_deadline = sleep(session.next_check_delay());
    tokio::pin!(session_deadline);

    loop {
        tokio::select! {
            // 安全/存活期限优先于持续可读的消息，防止高流量饿死撤销检查或 Ping。
            biased;
            changed = realtime_shutdown.changed() => {
                if changed.is_err() || *realtime_shutdown.borrow_and_update() {
                    close_connection(&mut socket, CLOSE_GOING_AWAY, REASON_SHUTDOWN).await;
                    return;
                }
            }
            _ = &mut session_deadline => {
                if !recheck_session(&mut socket, &mut session).await {
                    return;
                }
                session_deadline.as_mut().reset(Instant::now() + session.next_check_delay());
            }
            _ = &mut pong_deadline => return,
            _ = ticker.tick() => {
                if !send_message(&mut socket, Message::Ping(Vec::new().into())).await {
                    return;
                }
            }
            input = receive_input(&mut socket, &mut subscription) => match input {
                RealtimeInput::Client(message) => match message {
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Ok(Message::Ping(payload))) => {
                        if !send_message(&mut socket, Message::Pong(payload)).await {
                            return;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        pong_deadline.as_mut().reset(Instant::now() + PONG_WAIT);
                    }
                    Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => {
                        close_connection(
                            &mut socket,
                            CLOSE_POLICY_VIOLATION,
                            REASON_CLIENT_DATA,
                        )
                        .await;
                        return;
                    }
                    Some(Err(error)) => {
                        if is_message_too_long(error) {
                            // Tungstenite 已按 4 KiB 上限拒绝正文；仍发送旧 v1 约定的空原因 1009 close。
                            close_connection(&mut socket, CLOSE_MESSAGE_TOO_BIG, "").await;
                        }
                        return;
                    }
                },
                RealtimeInput::Event(event) => {
                    let Some(event) = event else {
                        close_connection(
                            &mut socket,
                            CLOSE_TRY_AGAIN_LATER,
                            REASON_SLOW_CONSUMER,
                        )
                        .await;
                        return;
                    };
                    if event.validate().is_err()
                        || event.stream_key != expected_stream
                        || matches!(target, RealtimeTarget::Discovery) && event.kind != "auction.created"
                    {
                        close_connection(
                            &mut socket,
                            CLOSE_PROTOCOL_ERROR,
                            REASON_PROTOCOL_ERROR,
                        )
                        .await;
                        return;
                    }
                    let message = match serde_json::to_string(&map_event(event)) {
                        Ok(message) => message,
                        Err(_) => return,
                    };
                    if !send_message(&mut socket, Message::Text(message.into())).await {
                        return;
                    }
                }
            },
        }
    }
}

enum RealtimeInput {
    Client(Option<Result<Message, axum::Error>>),
    Event(Option<PublicRealtimeEvent>),
}

async fn receive_input(
    socket: &mut WebSocket,
    subscription: &mut RealtimeSubscription,
) -> RealtimeInput {
    // 两个数据来源公平竞争；外层仍优先处理关闭、会话和存活期限。
    tokio::select! {
        message = socket.recv() => RealtimeInput::Client(message),
        event = subscription.recv() => RealtimeInput::Event(event),
    }
}

async fn recheck_session(socket: &mut WebSocket, session: &mut RealtimeSession) -> bool {
    let (code, reason) = match session.recheck().await {
        Ok(()) => return true,
        Err(SessionCheckError::Expired) => (CLOSE_POLICY_VIOLATION, REASON_SESSION_EXPIRED),
        Err(SessionCheckError::Unavailable) => (CLOSE_TRY_AGAIN_LATER, REASON_SESSION_UNAVAILABLE),
    };
    close_connection(socket, code, reason).await;
    false
}

async fn send_message(socket: &mut WebSocket, message: Message) -> bool {
    matches!(timeout(WRITE_WAIT, socket.send(message)).await, Ok(Ok(())))
}

fn is_message_too_long(error: axum::Error) -> bool {
    matches!(
        error.into_inner().downcast_ref::<tungstenite::Error>(),
        Some(tungstenite::Error::Capacity(
            tungstenite::error::CapacityError::MessageTooLong { .. }
        ))
    )
}

async fn close_connection(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let frame = CloseFrame { code, reason: reason.into() };
    let _ = send_message(socket, Message::Close(Some(frame))).await;
}
