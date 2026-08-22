//! Admin 邮件轮询与入站审查 HTTP 客户端。
//!
//! 浏览器只读取轮询状态和入站安全摘要；暂停和 A20 触发动作携带服务端快照中的 state version，
//! 并由后端生成新的命令 ID，浏览器不推导邮件处理状态。

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
    mailbox_key: String,
    expected_state_version: i64,
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
    mailbox_key: String,
    page: i64,
) -> Result<AdminMailIntakePage, String> {
    let encoded = encode_query(&mailbox_key);
    let response = Request::get(&format!(
        "/api/v1/admin/mail/intake?mailbox_key={encoded}&page={page}&page_size=50"
    ))
    .send()
    .await
    .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminMailIntakeListData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.pagination.page != page
        || payload.data.intakes.iter().any(|intake| intake.mailbox_key != mailbox_key)
    {
        return Err("mail intake response does not match request".to_owned());
    }
    Ok(payload.data.into())
}

pub(crate) async fn set_mail_paused(
    polling: AdminMailPolling,
    paused: bool,
) -> Result<AdminMailPolling, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::put("/api/v1/admin/mail/polling")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminMailPollingRequest {
            command_id: Uuid::new_v4(),
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

pub(crate) async fn trigger_mail_poll(
    polling: AdminMailPolling,
) -> Result<AdminMailPolling, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::post("/api/v1/admin/mail/polls:trigger")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminMailPollTriggerRequest {
            command_id: Uuid::new_v4(),
            mailbox_key: polling.mailbox_key.clone(),
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
        || payload.data.polling.state_version <= polling.state_version
    {
        return Err("mail polling trigger response does not match command".to_owned());
    }
    Ok(payload.data.polling)
}
