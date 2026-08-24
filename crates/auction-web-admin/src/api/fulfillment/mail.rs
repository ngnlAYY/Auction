//! Admin 邮件轮询与入站审查 HTTP 客户端。
//!
//! 浏览器只读取轮询状态和入站安全摘要；暂停动作携带服务端快照中的 state version，
//! A20 触发动作使用单个 command ID 唤醒全部 Worker poller，浏览器不推导邮件处理状态。

pub(crate) use auction_contracts::{
    AdminMailIntakeDetailResponse as AdminMailIntakeDetail,
    AdminMailIntakeListResponse as AdminMailIntakePage,
    AdminMailIntakeSummaryResponse as AdminMailIntake,
    AdminMailPollingResponse as AdminMailPolling,
};
use auction_contracts::{
    AdminMailPollTriggerRequest, AdminMailPollTriggerResponse, AdminMailPollingRequest,
    AdminMailStateResponse,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::ApiResponse;

fn encode_query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
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

pub(crate) async fn fetch_mail_state(mailbox_key: String) -> Result<AdminMailPolling, String> {
    let encoded = encode_query(&mailbox_key);
    let response = Request::get(&format!("/api/v1/admin/mail?mailbox_key={encoded}"))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminMailStateResponse> =
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
    let payload: ApiResponse<AdminMailIntakePage> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.pagination.page != page
        || (!query.mailbox_key.is_empty()
            && payload.data.intakes.iter().any(|intake| intake.mailbox_key != query.mailbox_key))
    {
        return Err("mail intake response does not match request".to_owned());
    }
    Ok(payload.data)
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
    let payload: ApiResponse<AdminMailStateResponse> =
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
    let payload: ApiResponse<AdminMailPollTriggerResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.command_id != command_id.to_string() || payload.data.pollers <= 0 {
        return Err("mail polling trigger response does not match command".to_owned());
    }
    Ok(())
}
