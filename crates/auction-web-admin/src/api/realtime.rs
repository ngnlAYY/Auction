//! Admin realtime 运维 API 客户端。
//!
//! 浏览器只接收 lag/failure 的安全投影；replay 的公开 payload 由管理操作员显式输入，
//! 并由服务端事件注册表、命令账本和数据库事务做最终校验。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::form_urlencoded;
use uuid::Uuid;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RealtimeLag {
    pub stream_key: String,
    pub consumer_key: String,
    pub offset: i64,
    pub latest_sequence: i64,
    pub lag: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct RealtimeLagListData {
    lags: Vec<RealtimeLag>,
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RealtimeLagPage {
    pub lags: Vec<RealtimeLag>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

impl From<RealtimeLagListData> for RealtimeLagPage {
    fn from(data: RealtimeLagListData) -> Self {
        Self { lags: data.lags, page: data.page, page_size: data.page_size, total: data.total }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RealtimeFailure {
    pub id: String,
    pub event_id: String,
    pub stream_key: String,
    pub consumer_key: String,
    pub code: String,
    pub safe_detail: String,
    pub failed_at: String,
    pub resolved_at: Option<String>,
    pub resolved: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct RealtimeFailureListData {
    failures: Vec<RealtimeFailure>,
    page: i64,
    page_size: i64,
    has_more: bool,
    total: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct RealtimeFailurePage {
    pub failures: Vec<RealtimeFailure>,
    pub page: i64,
    pub page_size: i64,
    pub has_more: bool,
    pub total: i64,
}

impl From<RealtimeFailureListData> for RealtimeFailurePage {
    fn from(data: RealtimeFailureListData) -> Self {
        Self {
            failures: data.failures,
            page: data.page,
            page_size: data.page_size,
            has_more: data.has_more,
            total: data.total,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct RealtimeReplayRequest {
    command_id: Uuid,
    schema_version: i32,
    public_payload: Value,
}

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
    let payload: ApiResponse<RealtimeLagListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
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
    let payload: ApiResponse<RealtimeFailureListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
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
        .json(&RealtimeReplayRequest { command_id, schema_version, public_payload })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let _: ApiResponse<Value> = response.json().await.map_err(|error| error.to_string())?;
    Ok(())
}
