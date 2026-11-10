use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::core::RequestHash;
use auction_application::realtime::RealtimeReplayCommand;
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
    AdminApiState, REPLAY_COMMAND_KIND, authenticated_admin_mutation, internal_error,
    invalid_replay, map_replay_error, no_store_headers, replay_unavailable, request_id,
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
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let actor_id = identity.actor;
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "replay 命令不接受查询参数",
        )
        .into_response();
    }
    let failure_id = match Uuid::parse_str(&failure_id) {
        Ok(value) if !value.is_nil() => value,
        _ => return invalid_replay(),
    };
    let input = match decode_json::<AdminRealtimeReplayRequest>(&body) {
        Ok(input) => input,
        Err(()) => return invalid_replay(),
    };
    if input.command_id.is_some_and(|command_id| command_id.is_nil())
        || input.schema_version <= 0
        || !input.public_payload.is_object()
    {
        return invalid_replay();
    }
    let command_id = match input.command_id {
        Some(command_id) => command_id,
        None => {
            match legacy_replay_command_id(failure_id, input.schema_version, &input.public_payload)
            {
                Ok(command_id) => command_id,
                Err(diagnostic_cause) => {
                    diagnostics::error(
                        "admin_realtime",
                        "derive_replay_command",
                        diagnostics::invalid_state(&diagnostic_cause),
                        DiagnosticContext::default(),
                    );
                    return internal_error();
                }
            }
        }
    };
    let hash_input = ReplayHashInput {
        failure_id,
        schema_version: input.schema_version,
        public_payload: &input.public_payload,
        actor_id: &actor_id,
    };
    let request_hash = match RequestHash::from_request(REPLAY_COMMAND_KIND, failure_id, &hash_input)
    {
        Ok(hash) => hash,
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_realtime",
                "hash_replay",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let Some(port) = state.realtime_admin() else {
        return replay_unavailable();
    };
    let result = match port
        .replay(RealtimeReplayCommand {
            command_id,
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
        Err(error) => return map_replay_error(error),
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

fn decode_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, ()> {
    serde_json::from_slice(body).map_err(|_| ())
}

#[derive(Debug, Serialize)]
struct ReplayHashInput<'a> {
    failure_id: Uuid,
    schema_version: i32,
    public_payload: &'a Value,
    actor_id: &'a str,
}

#[derive(Debug, Serialize)]
struct LegacyReplayCommandIdInput<'a> {
    failure_id: Uuid,
    schema_version: i32,
    public_payload: &'a Value,
}

/// 旧版 replay 请求没有 `command_id`。这里按 failure 与修正负载派生稳定 UUID，使同一旧版
/// 请求的网络重试仍命中命令账本；显式携带 ID 的新版客户端继续拥有自己的幂等键。
fn legacy_replay_command_id(
    failure_id: Uuid,
    schema_version: i32,
    public_payload: &Value,
) -> Result<Uuid, serde_json::Error> {
    let input = LegacyReplayCommandIdInput { failure_id, schema_version, public_payload };
    let mut name = b"auction/http/admin/realtime/replay/v1/".to_vec();
    // 直接追加紧凑 JSON，保留原 UUID 输入字节，无需暂存并复制整份 payload。
    serde_json::to_writer(&mut name, &input)?;
    Ok(Uuid::new_v5(&Uuid::NAMESPACE_URL, &name))
}
