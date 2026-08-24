use std::net::SocketAddr;

use auction_application::{RealtimeReplayCommand, RequestHash};
use auction_contracts::{
    AdminRealtimeReplayEventResponse, AdminRealtimeReplayRequest, AdminRealtimeReplayResponse,
    ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use uuid::Uuid;

use super::{
    ADMIN_CSRF_COOKIE, AdminApiState, REPLAY_COMMAND_KIND, admin_session_guard,
    authenticated_admin_session, authentication_error, feature_unavailable, internal_error,
    map_realtime_error, no_store_headers, parse_uuid, read_admin_cookie, request_id,
    valid_admin_csrf_header,
};

/// 追加一个 realtime replay 事件并解决原 failure。
pub(crate) async fn replay(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(failure_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "replay 命令不接受查询参数",
        )
        .into_response();
    }
    if let Some(response) = validate_csrf(&headers) {
        return response;
    }
    let failure_id = match parse_uuid(&failure_id, "failure_id") {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let input = match decode_json::<AdminRealtimeReplayRequest>(&body) {
        Ok(input) => input,
        Err(response) => return *response,
    };
    if input.command_id.is_nil() || input.schema_version <= 0 || !input.public_payload.is_object() {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_realtime_command",
            "replay 命令参数无效",
        )
        .into_response();
    }
    let (_, identity) = match authenticated_admin_session(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let actor_id = identity.actor;
    let hash_input = ReplayHashInput {
        failure_id,
        schema_version: input.schema_version,
        public_payload: &input.public_payload,
        actor_id: &actor_id,
    };
    let request_hash = match RequestHash::from_request(REPLAY_COMMAND_KIND, failure_id, &hash_input)
    {
        Ok(hash) => hash,
        Err(_) => return internal_error(),
    };
    let Some(port) = state.realtime_admin() else {
        return feature_unavailable();
    };
    let result = match port
        .replay(RealtimeReplayCommand {
            command_id: input.command_id,
            failure_id,
            schema_version: input.schema_version,
            public_payload: input.public_payload,
            actor_id,
            request_id: request_id(&headers),
            request_hash,
            created_at: Utc::now(),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_realtime_error(error),
    };
    let replay = AdminRealtimeReplayEventResponse {
        event_id: result.event_id.to_string(),
        replay_of_event_id: result.replay_of_event_id.to_string(),
        stream_key: result.stream_key,
        stream_sequence: result.stream_sequence,
        kind: result.kind,
        schema_version: result.schema_version,
        created_at: super::timestamp(result.created_at),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("实时事件已重放", AdminRealtimeReplayResponse { replay })),
    )
        .into_response()
}

fn decode_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, Box<Response>> {
    serde_json::from_slice(body).map_err(|_| {
        Box::new(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response(),
        )
    })
}

fn validate_csrf(headers: &HeaderMap) -> Option<Response> {
    let Some(raw_csrf) = read_admin_cookie(headers, ADMIN_CSRF_COOKIE) else {
        return Some(authentication_error());
    };
    if !valid_admin_csrf_header(headers, &raw_csrf) {
        return Some(
            super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
                .into_response(),
        );
    }
    None
}

#[derive(Debug, Serialize)]
struct ReplayHashInput<'a> {
    failure_id: Uuid,
    schema_version: i32,
    public_payload: &'a Value,
    actor_id: &'a str,
}
