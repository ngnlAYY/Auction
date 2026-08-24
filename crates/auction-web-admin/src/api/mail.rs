//! Admin 邮件轮询与入站审查 HTTP 客户端。
//!
//! 浏览器只读取轮询状态和入站安全摘要；暂停动作携带服务端快照中的 state version，
//! A20 触发动作使用单个 command ID 唤醒全部 Worker poller，浏览器不推导邮件处理状态。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ApiResponse;

fn encode_query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminMailPolling {
    pub mailbox_key: String,
    pub last_poll_at: String,
    pub next_poll_at: String,
    pub consecutive_failures: i32,
    pub paused: bool,
    pub state_version: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminMailStateData {
    polling: AdminMailPolling,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminMailIntake {
    pub id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub kind: String,
    pub status: String,
    pub decision_code: String,
    pub received_at: String,
    pub processed_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminMailPagination {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminMailIntakeListData {
    intakes: Vec<AdminMailIntake>,
    pagination: AdminMailPagination,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminMailIntakePage {
    pub intakes: Vec<AdminMailIntake>,
    pub pagination: AdminMailPagination,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AdminMailIntakeQuery {
    pub mailbox_key: String,
    pub world_id: String,
    pub kind: String,
    pub status: String,
    pub decision: String,
    pub received_from: String,
    pub received_to: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminMailIntakeMetadata {
    pub auction_id: String,
    pub submission_id: String,
    pub user_id: String,
    pub added_count: i32,
    pub existing_count: i32,
    pub preview_revision: i64,
    pub attachment_count: i32,
    pub owner_result_code: String,
    pub reason_code: String,
    pub warning_codes: Vec<String>,
    pub delivery_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminMailIntakeDetail {
    pub id: String,
    pub mailbox_key: String,
    pub external_mail_id: String,
    pub world_id: String,
    pub kind: String,
    pub status: String,
    pub decision_code: String,
    pub received_at: String,
    pub processed_at: String,
    pub metadata: AdminMailIntakeMetadata,
}

impl From<AdminMailIntakeListData> for AdminMailIntakePage {
    fn from(data: AdminMailIntakeListData) -> Self {
        Self { intakes: data.intakes, pagination: data.pagination }
    }
}

#[derive(Debug, Clone, Serialize)]
struct AdminMailPollingRequest {
    command_id: Uuid,
    mailbox_key: String,
    paused: bool,
    expected_state_version: i64,
}

#[derive(Debug, Clone, Serialize)]
struct AdminMailPollTriggerRequest {
    command_id: Uuid,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminMailPollTriggerData {
    command_id: String,
    pollers: i64,
}

pub(crate) async fn fetch_mail_state(mailbox_key: String) -> Result<AdminMailPolling, String> {
    let encoded = encode_query(&mailbox_key);
    let response = Request::get(&format!("/api/v1/admin/mail?mailbox_key={encoded}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminMailStateData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.polling.mailbox_key != mailbox_key {
        return Err("mail polling response does not match mailbox".to_owned());
    }
    Ok(payload.data.polling)
}

pub(crate) async fn fetch_mail_intakes(
    query: AdminMailIntakeQuery,
    page: i64,
) -> Result<AdminMailIntakePage, String> {
    let mut url = format!("/api/v1/admin/mail/intake?page={page}&page_size=20");
    append_query(&mut url, "mailbox_key", &query.mailbox_key);
    append_query(&mut url, "world_id", &query.world_id);
    append_query(&mut url, "kind", &query.kind);
    append_query(&mut url, "status", &query.status);
    append_query(&mut url, "decision", &query.decision);
    append_query(&mut url, "from", &normalize_datetime(&query.received_from));
    append_query(&mut url, "to", &normalize_datetime(&query.received_to));
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminMailIntakeListData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.pagination.page != page
        || (!query.mailbox_key.is_empty()
            && payload.data.intakes.iter().any(|intake| intake.mailbox_key != query.mailbox_key))
    {
        return Err("mail intake response does not match request".to_owned());
    }
    Ok(payload.data.into())
}

pub(crate) async fn fetch_mail_intake_detail(
    intake_id: String,
) -> Result<AdminMailIntakeDetail, String> {
    let response = Request::get(&format!("/api/v1/admin/mail/intake/{intake_id}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminMailIntakeDetail> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.id != intake_id {
        return Err("mail intake detail response does not match request".to_owned());
    }
    Ok(payload.data)
}

fn append_query(url: &mut String, key: &str, value: &str) {
    if !value.is_empty() {
        url.push('&');
        url.push_str(key);
        url.push('=');
        url.push_str(&encode_query(value));
    }
}

fn normalize_datetime(value: &str) -> String {
    super::normalize_datetime(value)
}

pub(crate) async fn set_mail_paused(
    command_id: Uuid,
    polling: AdminMailPolling,
    paused: bool,
) -> Result<AdminMailPolling, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::put("/api/v1/admin/mail/polling")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminMailPollingRequest {
            command_id,
            mailbox_key: polling.mailbox_key.clone(),
            paused,
            expected_state_version: polling.state_version,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminMailStateData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.polling.mailbox_key != polling.mailbox_key
        || payload.data.polling.paused != paused
        || payload.data.polling.state_version <= polling.state_version
    {
        return Err("mail polling update response does not match command".to_owned());
    }
    Ok(payload.data.polling)
}

pub(crate) async fn trigger_mail_poll(command_id: Uuid) -> Result<(), String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::post("/api/v1/admin/mail/polls:trigger")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminMailPollTriggerRequest { command_id })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminMailPollTriggerData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.command_id != command_id.to_string() || payload.data.pollers <= 0 {
        return Err("mail polling trigger response does not match command".to_owned());
    }
    Ok(())
}
