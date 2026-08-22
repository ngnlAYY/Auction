//! Admin realtime 运维 API 客户端。
//!
//! 浏览器只接收 lag/failure 的安全投影；replay 的公开 payload 由管理操作员显式输入，
//! 并由服务端事件注册表、命令账本和数据库事务做最终校验。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

pub(crate) async fn fetch_realtime_lag() -> Result<RealtimeLagPage, String> {
    let response = Request::get("/api/v1/admin/realtime/lag?page=1&page_size=100")
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

pub(crate) async fn fetch_realtime_failures() -> Result<RealtimeFailurePage, String> {
    let response =
        Request::get("/api/v1/admin/realtime/failures?resolved=false&page=1&page_size=100")
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
