//! Admin realtime 运维 API 客户端。
//! 浏览器只接收事件元数据与 lag/failure 的安全投影；replay 的公开 payload 由管理操作员
//! 显式输入，并由服务端事件注册表、命令账本和数据库事务做最终校验。

pub(crate) use auction_contracts::{
    AdminRealtimeEventListResponse as RealtimeEventPage,
    AdminRealtimeFailureListResponse as RealtimeFailurePage,
    AdminRealtimeFailureResponse as RealtimeFailure,
    AdminRealtimeLagListResponse as RealtimeLagPage,
};
use auction_contracts::{
    AdminRealtimeEventResponse, AdminRealtimeFailurePaginationResponse, AdminRealtimeLagResponse,
    AdminRealtimePaginationResponse, AdminRealtimeReplayRequest,
};
use gloo_net::http::Request;
use serde::Deserialize;
use serde_json::Value;
use url::form_urlencoded;
use uuid::Uuid;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, OptionalField, invalid_request_error,
    invalid_response_error, response_error, send_get, valid_admin_time, valid_safe_text,
    valid_trimmed_text, valid_uuid,
};

// 保留镜像的理由：旧 realtime decoder 允许对象扩展字段，contracts 的
// `AdminRealtimeEventListResponse`/`AdminRealtimeLagListResponse`/
// `AdminRealtimeFailureListResponse`/`AdminRealtimeReplayResponse` 不能加
// `deny_unknown_fields`；下方镜像继续要求全部旧字段显式存在并保留组合语义校验。
// 同步义务：上述 DTO 字段集变化时必须同步本文件；decode 函数中构造 DTO 的穷举字段列表
// 会在编译期捕捉漂移。
#[derive(Debug, Deserialize)]
struct StrictEvent {
    created_at: String,
    event_id: String,
    kind: String,
    schema_version: i32,
    stream_key: String,
    stream_sequence: i64,
}

#[derive(Debug, Deserialize)]
struct StrictEventPage {
    events: Vec<StrictEvent>,
    pagination: StrictPagination,
}

/// 对应 contracts DTO：`AdminRealtimeLagResponse`。
#[derive(Debug, Deserialize)]
struct StrictLag {
    consumer_key: String,
    lag: i64,
    latest_sequence: i64,
    offset: i64,
    stream_key: String,
}

#[derive(Debug, Deserialize)]
struct StrictPagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
struct StrictLagPage {
    lags: Vec<StrictLag>,
    pagination: StrictPagination,
}

#[derive(Debug, Deserialize)]
struct StrictFailure {
    code: String,
    consumer_key: String,
    event_id: String,
    failed_at: String,
    id: String,
    resolved: bool,
    #[serde(default)]
    resolved_at: OptionalField<String>,
    safe_detail: String,
    stream_key: String,
}

#[derive(Debug, Deserialize)]
struct StrictFailurePagination {
    has_more: bool,
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
struct StrictFailurePage {
    failures: Vec<StrictFailure>,
    pagination: StrictFailurePagination,
}

#[derive(Debug, Deserialize)]
struct StrictReplayData {
    replay: StrictReplay,
}

#[derive(Debug, Deserialize)]
struct StrictReplay {
    created_at: String,
    event_id: String,
    kind: String,
    replay_of_event_id: String,
    schema_version: i32,
    stream_key: String,
    stream_sequence: i64,
}

pub(crate) async fn fetch_realtime_event_page(
    page: i64,
    stream_key: String,
    kind: String,
    page_size: i64,
) -> Result<RealtimeEventPage, String> {
    fetch_realtime_event_page_with_size(page, page_size, stream_key, kind).await
}

/// 只取全局事件总数，避免概览卡片被列表筛选条件改写。
pub(crate) async fn fetch_realtime_event_total() -> Result<i64, String> {
    let page = fetch_realtime_event_page_with_size(1, 1, String::new(), String::new()).await?;
    Ok(page.pagination.total)
}

async fn fetch_realtime_event_page_with_size(
    page: i64,
    page_size: i64,
    stream_key: String,
    kind: String,
) -> Result<RealtimeEventPage, String> {
    super::pagination::validate(page, page_size, 200, 1_000_000)?;
    let requested_page = page;
    let requested_page_size = page_size.max(1);
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &requested_page.to_string());
    query.append_pair("page_size", &requested_page_size.to_string());
    append_filter(&mut query, "stream_key", &stream_key);
    append_filter(&mut query, "kind", &kind);
    let response = send_get(&format!("/api/v1/admin/realtime/events?{}", query.finish())).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictEventPage> =
        response.json().await.map_err(invalid_response_error)?;
    decode_event_page(
        payload.data,
        requested_page,
        requested_page_size,
        stream_key.trim(),
        kind.trim(),
    )
    .map_err(invalid_response_error)
}

pub(crate) async fn fetch_realtime_lag_page(
    page: i64,
    stream_prefix: String,
    consumer_key: String,
    page_size: i64,
) -> Result<RealtimeLagPage, String> {
    fetch_realtime_lag_page_with_size(page, page_size, stream_prefix, consumer_key).await
}

async fn fetch_realtime_lag_page_with_size(
    page: i64,
    page_size: i64,
    stream_prefix: String,
    consumer_key: String,
) -> Result<RealtimeLagPage, String> {
    super::pagination::validate(page, page_size, 200, 1_000_000)?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string());
    query.append_pair("page_size", &page_size.max(1).to_string());
    append_filter(&mut query, "stream_prefix", &stream_prefix);
    append_filter(&mut query, "consumer_key", &consumer_key);
    let response = send_get(&format!("/api/v1/admin/realtime/lag?{}", query.finish())).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictLagPage> =
        response.json().await.map_err(invalid_response_error)?;
    decode_lag_page(
        payload.data,
        page.max(1),
        page_size.max(1),
        stream_prefix.trim(),
        consumer_key.trim(),
    )
    .map_err(invalid_response_error)
}

pub(crate) async fn fetch_realtime_failure_page(
    page: i64,
    stream_key: String,
    consumer_key: String,
    code: String,
    state: String,
    page_size: i64,
) -> Result<RealtimeFailurePage, String> {
    fetch_realtime_failure_page_with_size(page, page_size, stream_key, consumer_key, code, state)
        .await
}

async fn fetch_realtime_failure_page_with_size(
    page: i64,
    page_size: i64,
    stream_key: String,
    consumer_key: String,
    code: String,
    state: String,
) -> Result<RealtimeFailurePage, String> {
    super::pagination::validate(page, page_size, 200, 1_000_000)?;
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string());
    query.append_pair("page_size", &page_size.max(1).to_string());
    append_filter(&mut query, "stream_key", &stream_key);
    append_filter(&mut query, "consumer_key", &consumer_key);
    append_filter(&mut query, "code", &code);
    if let Some(resolved) = resolved_from_legacy_state(&state) {
        query.append_pair("resolved", if resolved { "true" } else { "false" });
    }
    let response = send_get(&format!("/api/v1/admin/realtime/failures?{}", query.finish())).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictFailurePage> =
        response.json().await.map_err(invalid_response_error)?;
    decode_failure_page(
        payload.data,
        page.max(1),
        page_size.max(1),
        stream_key.trim(),
        consumer_key.trim(),
        code.trim(),
        resolved_from_legacy_state(&state),
    )
    .map_err(invalid_response_error)
}

/// 将旧版 Select 的状态 token 转换为后端的布尔筛选；未知值安全地视为未筛选。
fn resolved_from_legacy_state(state: &str) -> Option<bool> {
    match state {
        "dead_letter" => Some(false),
        "completed" => Some(true),
        // 空值、pending、retry 与未知值都不筛选。
        _ => None,
    }
}

fn append_filter(query: &mut form_urlencoded::Serializer<String>, key: &str, value: &str) {
    if !value.trim().is_empty() {
        query.append_pair(key, value.trim());
    }
}

pub(crate) async fn replay_realtime_failure(
    failure_id: String,
    command_id: Uuid,
    schema_version: i32,
    public_payload: String,
) -> Result<(), String> {
    let public_payload: Value =
        serde_json::from_str(&public_payload).map_err(invalid_request_error)?;
    if !public_payload.is_object() {
        return Err("invalid_request".to_owned());
    }
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/realtime/failures/{failure_id}/replay");
    let command =
        AdminRealtimeReplayRequest { command_id: Some(command_id), schema_version, public_payload };
    let response = super::send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictReplayData> =
        response.json().await.map_err(invalid_response_error)?;
    decode_replay(payload.data, schema_version).map_err(invalid_response_error)?;
    Ok(())
}

fn decode_lag_page(
    data: StrictLagPage,
    requested_page: i64,
    requested_page_size: i64,
    stream_prefix: &str,
    consumer_key: &str,
) -> Result<RealtimeLagPage, &'static str> {
    let pagination = decode_pagination(data.pagination)?;
    if pagination.page != requested_page
        || pagination.page_size != requested_page_size
        || i64::try_from(data.lags.len())
            .map_or(true, |count| count > pagination.page_size || count > pagination.total)
    {
        return Err("realtime_lag_page");
    }
    let lags = data.lags.into_iter().map(decode_lag).collect::<Result<Vec<_>, _>>()?;
    if lags.iter().any(|lag| {
        (!stream_prefix.is_empty() && !lag.stream_key.starts_with(stream_prefix))
            || (!consumer_key.is_empty() && lag.consumer_key != consumer_key)
    }) {
        return Err("realtime_lag_filter");
    }
    Ok(RealtimeLagPage { lags, pagination })
}

fn decode_event_page(
    data: StrictEventPage,
    requested_page: i64,
    requested_page_size: i64,
    stream_key: &str,
    kind: &str,
) -> Result<RealtimeEventPage, &'static str> {
    let pagination = decode_pagination(data.pagination)?;
    if pagination.page != requested_page
        || pagination.page_size != requested_page_size
        || i64::try_from(data.events.len())
            .map_or(true, |count| count > pagination.page_size || count > pagination.total)
    {
        return Err("realtime_event_page");
    }
    let events = data.events.into_iter().map(decode_event).collect::<Result<Vec<_>, _>>()?;
    if events.iter().any(|event| {
        (!stream_key.is_empty() && event.stream_key != stream_key)
            || (!kind.is_empty() && event.kind != kind)
    }) {
        return Err("realtime_event_filter");
    }
    Ok(RealtimeEventPage { events, pagination })
}

fn decode_event(value: StrictEvent) -> Result<AdminRealtimeEventResponse, &'static str> {
    if !valid_uuid(&value.event_id)
        || !valid_safe_text(&value.stream_key, 256)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.stream_sequence)
        || !valid_safe_text(&value.kind, 128)
        || value.schema_version <= 0
        || !valid_admin_time(&value.created_at)
    {
        return Err("realtime_event");
    }
    Ok(AdminRealtimeEventResponse {
        event_id: value.event_id,
        stream_key: value.stream_key,
        stream_sequence: value.stream_sequence,
        kind: value.kind,
        schema_version: value.schema_version,
        created_at: value.created_at,
    })
}

fn decode_lag(value: StrictLag) -> Result<AdminRealtimeLagResponse, &'static str> {
    if !valid_trimmed_text(&value.consumer_key)
        || !valid_trimmed_text(&value.stream_key)
        || !valid_count(value.offset)
        || !valid_count(value.latest_sequence)
        || !valid_count(value.lag)
    {
        return Err("realtime_lag");
    }
    Ok(AdminRealtimeLagResponse {
        stream_key: value.stream_key,
        consumer_key: value.consumer_key,
        offset: value.offset,
        latest_sequence: value.latest_sequence,
        lag: value.lag,
    })
}

fn decode_failure_page(
    data: StrictFailurePage,
    requested_page: i64,
    requested_page_size: i64,
    stream_key: &str,
    consumer_key: &str,
    code: &str,
    resolved: Option<bool>,
) -> Result<RealtimeFailurePage, &'static str> {
    let pagination = data.pagination;
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&pagination.page)
        || pagination.page != requested_page
        || pagination.page_size != requested_page_size
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&pagination.page_size)
        || !valid_count(pagination.total)
        || i64::try_from(data.failures.len())
            .map_or(true, |count| count > pagination.page_size || count > pagination.total)
        || pagination.has_more
            != pagination
                .page
                .checked_mul(pagination.page_size)
                .is_some_and(|end| end < pagination.total)
    {
        return Err("realtime_failure_page");
    }
    let failures = data.failures.into_iter().map(decode_failure).collect::<Result<Vec<_>, _>>()?;
    if failures.iter().any(|failure| {
        (!stream_key.is_empty() && failure.stream_key != stream_key)
            || (!consumer_key.is_empty() && failure.consumer_key != consumer_key)
            || (!code.is_empty() && failure.code != code)
            || resolved.is_some_and(|resolved| failure.resolved != resolved)
    }) {
        return Err("realtime_failure_filter");
    }
    Ok(RealtimeFailurePage {
        failures,
        pagination: AdminRealtimeFailurePaginationResponse {
            page: pagination.page,
            page_size: pagination.page_size,
            has_more: pagination.has_more,
            total: pagination.total,
        },
    })
}

fn decode_failure(value: StrictFailure) -> Result<RealtimeFailure, &'static str> {
    if !valid_uuid(&value.id)
        || !valid_uuid(&value.event_id)
        || !valid_trimmed_text(&value.stream_key)
        || !valid_trimmed_text(&value.consumer_key)
        || !valid_trimmed_text(&value.code)
        || !valid_safe_text(&value.safe_detail, 2_048)
        || !valid_admin_time(&value.failed_at)
        || value.resolved_at.0.as_deref().is_some_and(|time| !valid_admin_time(time))
        || value.resolved != value.resolved_at.0.is_some()
    {
        return Err("realtime_failure");
    }
    Ok(RealtimeFailure {
        id: value.id,
        event_id: value.event_id,
        stream_key: value.stream_key,
        consumer_key: value.consumer_key,
        code: value.code,
        safe_detail: value.safe_detail,
        failed_at: value.failed_at,
        resolved_at: value.resolved_at.0,
        resolved: value.resolved,
    })
}

fn decode_replay(data: StrictReplayData, requested_schema: i32) -> Result<(), &'static str> {
    let replay = data.replay;
    if !valid_uuid(&replay.event_id)
        || !valid_uuid(&replay.replay_of_event_id)
        || replay.event_id == replay.replay_of_event_id
        || !valid_trimmed_text(&replay.stream_key)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&replay.stream_sequence)
        || !valid_trimmed_text(&replay.kind)
        || replay.schema_version <= 0
        || replay.schema_version != requested_schema
        || !valid_admin_time(&replay.created_at)
    {
        return Err("realtime_replay");
    }
    Ok(())
}

fn decode_pagination(
    value: StrictPagination,
) -> Result<AdminRealtimePaginationResponse, &'static str> {
    if !crate::api::pagination::valid_response(value.page, value.page_size, value.total) {
        return Err("realtime_pagination");
    }
    Ok(AdminRealtimePaginationResponse {
        page: value.page,
        page_size: value.page_size,
        total: value.total,
    })
}

fn valid_count(value: i64) -> bool {
    (0..=MAX_JSON_SAFE_INTEGER).contains(&value)
}
