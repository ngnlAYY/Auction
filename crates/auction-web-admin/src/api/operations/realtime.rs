//! Admin realtime 运维 API 客户端。
//!
//! 浏览器只接收 lag/failure 的安全投影；replay 的公开 payload 由管理操作员显式输入，
//! 并由服务端事件注册表、命令账本和数据库事务做最终校验。

pub(crate) use auction_contracts::{
    AdminRealtimeFailureListResponse as RealtimeFailurePage,
    AdminRealtimeFailureResponse as RealtimeFailure,
    AdminRealtimeLagListResponse as RealtimeLagPage,
};
use auction_contracts::{AdminRealtimeReplayRequest, AdminRealtimeReplayResponse};
use gloo_net::http::Request;
use serde_json::Value;
use url::form_urlencoded;
use uuid::Uuid;

use super::ApiResponse;

pub(crate) async fn fetch_realtime_lag_page(
    page: i64,
    stream_prefix: String,
    consumer_key: String,
) -> Result<RealtimeLagPage, String> {
    fetch_realtime_lag_page_with_size(page, 50, stream_prefix, consumer_key).await
}

async fn fetch_realtime_lag_page_with_size(
    page: i64,
    page_size: i64,
    stream_prefix: String,
    consumer_key: String,
) -> Result<RealtimeLagPage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string());
    query.append_pair("page_size", &page_size.max(1).to_string());
    append_filter(&mut query, "stream_prefix", &stream_prefix);
    append_filter(&mut query, "consumer_key", &consumer_key);
    let response = Request::get(&format!("/api/v1/admin/realtime/lag?{}", query.finish()))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<RealtimeLagPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_realtime_failure_page(
    page: i64,
    stream_key: String,
    consumer_key: String,
    code: String,
    resolved: Option<bool>,
) -> Result<RealtimeFailurePage, String> {
    fetch_realtime_failure_page_with_size(page, 50, stream_key, consumer_key, code, resolved).await
}

async fn fetch_realtime_failure_page_with_size(
    page: i64,
    page_size: i64,
    stream_key: String,
    consumer_key: String,
    code: String,
    resolved: Option<bool>,
) -> Result<RealtimeFailurePage, String> {
    let mut query = form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.max(1).to_string());
    query.append_pair("page_size", &page_size.max(1).to_string());
    append_filter(&mut query, "stream_key", &stream_key);
    append_filter(&mut query, "consumer_key", &consumer_key);
    append_filter(&mut query, "code", &code);
    if let Some(resolved) = resolved {
        query.append_pair("resolved", if resolved { "true" } else { "false" });
    }
    let response = Request::get(&format!("/api/v1/admin/realtime/failures?{}", query.finish()))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<RealtimeFailurePage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
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
        serde_json::from_str(&public_payload).map_err(|error| error.to_string())?;
    if !public_payload.is_object() {
        return Err("replay payload must be an object".to_owned());
    }
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::post(&format!("/api/v1/admin/realtime/failures/{failure_id}/replay"))
        .header("x-csrf-token", &csrf_token)
        .json(&AdminRealtimeReplayRequest { command_id, schema_version, public_payload })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let _: ApiResponse<AdminRealtimeReplayResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(())
}
