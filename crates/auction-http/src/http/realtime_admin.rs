//! Admin realtime 运维 HTTP 适配器。
//!
//! 列表和详情只返回 lag/failure 的安全投影；replay 必须携带稳定 `command_id`、通过 CSRF，
//! 并把请求摘要、审计身份和公开 payload 交给应用端口与数据库事务 owner。

use std::net::SocketAddr;

use auction_application::{
    ApplicationError, MAX_REALTIME_ADMIN_PAGE_SIZE, RealtimeAdminError, RealtimeFailureListRequest,
    RealtimeFailureSnapshot, RealtimeLagListRequest, RealtimeLagSnapshot, RealtimeReplayCommand,
    RequestHash,
};
use auction_contracts::{
    AdminRealtimeFailureDetailResponse, AdminRealtimeFailureListResponse,
    AdminRealtimeFailureResponse, AdminRealtimeLagListResponse, AdminRealtimeLagResponse,
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

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;
const REPLAY_COMMAND_KIND: &str = "replay_realtime_failure";

/// 读取 realtime consumer lag。
pub(crate) async fn list_lag(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query =
        match super::parse_query(&uri, &["stream_prefix", "consumer_key", "page", "page_size"]) {
            Ok(query) => query,
            Err(_) => return invalid_query(),
        };
    let (page, page_size, offset) = match parse_page(&query) {
        Ok(page) => page,
        Err(_) => return invalid_query(),
    };
    let Some(port) = state.realtime_admin() else {
        return feature_unavailable();
    };
    let result = port
        .list_lag(RealtimeLagListRequest {
            stream_prefix: query.get("stream_prefix").cloned(),
            consumer_key: query.get("consumer_key").cloned(),
            limit: page_size,
            offset,
        })
        .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => return map_realtime_error(error),
    };
    if result.limit != page_size
        || result.offset != offset
        || result.total < 0
        || result.entries.len() as i64 > page_size
    {
        return super::internal_error();
    }
    let lags = result.entries.into_iter().map(map_lag).collect();
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "实时消费延迟已加载",
            AdminRealtimeLagListResponse { lags, page, page_size, total: result.total },
        )),
    )
        .into_response()
}

/// 读取 realtime consumer failure 列表。
pub(crate) async fn list_failures(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let query = match super::parse_query(
        &uri,
        &["stream_key", "consumer_key", "code", "resolved", "page", "page_size"],
    ) {
        Ok(query) => query,
        Err(_) => return invalid_query(),
    };
    let resolved = match query.get("resolved") {
        None => None,
        Some(value) => Some(match value.as_str() {
            "true" => true,
            "false" => false,
            _ => return invalid_query(),
        }),
    };
    let (page, page_size, offset) = match parse_page(&query) {
        Ok(page) => page,
        Err(_) => return invalid_query(),
    };
    let Some(port) = state.realtime_admin() else {
        return feature_unavailable();
    };
    let result = port
        .list_failures(RealtimeFailureListRequest {
            stream_key: query.get("stream_key").cloned(),
            consumer_key: query.get("consumer_key").cloned(),
            code: query.get("code").cloned(),
            resolved,
            limit: page_size,
            offset,
        })
        .await;
    let result = match result {
        Ok(result) => result,
        Err(error) => return map_realtime_error(error),
    };
    if result.limit != page_size
        || result.offset != offset
        || result.total < 0
        || result.entries.len() as i64 > page_size
    {
        return super::internal_error();
    }
    let has_more =
        offset.checked_add(result.entries.len() as i64).is_some_and(|next| next < result.total);
    let failures = result.entries.into_iter().map(map_failure).collect();
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "实时消费失败列表已加载",
            AdminRealtimeFailureListResponse {
                failures,
                page,
                page_size,
                has_more,
                total: result.total,
            },
        )),
    )
        .into_response()
}

/// 读取单个 realtime failure 的安全详情。
pub(crate) async fn get_failure(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(failure_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    let failure_id = match parse_uuid(&failure_id, "failure_id") {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let Some(port) = state.realtime_admin() else {
        return feature_unavailable();
    };
    let failure = match port.get_failure(failure_id).await {
        Ok(Some(failure)) => failure,
        Ok(None) => return super::not_found_error().into_response(),
        Err(error) => return map_realtime_error(error),
    };
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "实时消费失败详情已加载",
            AdminRealtimeFailureDetailResponse { failure: map_failure(failure) },
        )),
    )
        .into_response()
}

/// 追加一个 realtime replay 事件并解决原 failure。
pub(crate) async fn replay(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(failure_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
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
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
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
        Err(_) => return super::internal_error(),
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
            request_id: None,
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
        super::no_store_headers(),
        Json(ApiResponse::ok("实时事件已重放", AdminRealtimeReplayResponse { replay })),
    )
        .into_response()
}

fn parse_page(query: &std::collections::BTreeMap<String, String>) -> Result<(i64, i64, i64), ()> {
    let page = super::parse_positive_i64(query.get("page"), 1, None)?;
    let page_size = super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_REALTIME_ADMIN_PAGE_SIZE),
    )?;
    let offset = page.checked_sub(1).and_then(|value| value.checked_mul(page_size)).ok_or(())?;
    Ok((page, page_size, offset))
}

fn parse_uuid(value: &str, field: &'static str) -> Result<Uuid, Box<Response>> {
    match Uuid::parse_str(value) {
        Ok(value) if !value.is_nil() => Ok(value),
        _ => Err(Box::new(
            super::api_error(
                StatusCode::BAD_REQUEST,
                "invalid_id",
                match field {
                    "failure_id" => "failure ID 无效",
                    _ => "ID 无效",
                },
            )
            .into_response(),
        )),
    }
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
    let Some(raw_csrf) = super::read_admin_cookie(headers, super::ADMIN_CSRF_COOKIE) else {
        return Some(super::authentication_error());
    };
    if !super::valid_admin_csrf_header(headers, &raw_csrf) {
        return Some(
            super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
                .into_response(),
        );
    }
    None
}

fn map_lag(lag: RealtimeLagSnapshot) -> AdminRealtimeLagResponse {
    AdminRealtimeLagResponse {
        stream_key: lag.stream_key,
        consumer_key: lag.consumer_key,
        offset: lag.offset,
        latest_sequence: lag.latest_sequence,
        lag: lag.lag,
    }
}

fn map_failure(failure: RealtimeFailureSnapshot) -> AdminRealtimeFailureResponse {
    let resolved = failure.resolved();
    AdminRealtimeFailureResponse {
        id: failure.id.to_string(),
        event_id: failure.event_id.to_string(),
        stream_key: failure.stream_key,
        consumer_key: failure.consumer_key,
        code: failure.code,
        safe_detail: failure.safe_detail,
        failed_at: super::timestamp(failure.failed_at),
        resolved_at: failure.resolved_at.map(super::timestamp),
        resolved,
    }
}

fn map_realtime_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::RealtimeAdmin(RealtimeAdminError::InvalidCommand) => super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_realtime_command",
            "realtime 管理参数无效",
        )
        .into_response(),
        ApplicationError::RealtimeAdmin(RealtimeAdminError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::RealtimeAdmin(RealtimeAdminError::AlreadyResolved) => super::api_error(
            StatusCode::CONFLICT,
            "realtime_failure_resolved",
            "realtime failure 已经解决",
        )
        .into_response(),
        ApplicationError::RealtimeAdmin(RealtimeAdminError::ReplayMismatch) => super::api_error(
            StatusCode::CONFLICT,
            "realtime_replay_mismatch",
            "replay 与原 realtime failure 不匹配",
        )
        .into_response(),
        ApplicationError::RealtimeAdmin(RealtimeAdminError::CommandConflict) => {
            super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "realtime 管理服务暂不可用",
        )
        .into_response(),
    }
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "realtime Admin 依赖当前数据库档位未提供",
    )
    .into_response()
}

fn invalid_query() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response()
}

#[derive(Debug, Serialize)]
struct ReplayHashInput<'a> {
    failure_id: Uuid,
    schema_version: i32,
    public_payload: &'a Value,
    actor_id: &'a str,
}
