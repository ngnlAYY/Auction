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
    AdminMailIntakeMetadataResponse, AdminMailPaginationResponse, AdminMailPollTriggerRequest,
    AdminMailPollingRequest,
};
use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    AdminApiFailure, ApiResponse, MAX_JSON_SAFE_INTEGER, TABLE_PAGE_SIZE, invalid_request_error,
    invalid_response_error, parse_admin_time, response_error, response_failure, send_get,
    valid_admin_time, valid_safe_text, valid_trimmed_text, valid_uuid,
};

/// 旧 Mail decoder 允许非破坏性扩展字段；所有旧字段及新详情的固定 metadata 仍逐项校验。
#[derive(Debug, Deserialize)]
struct StrictMailState {
    polling: StrictMailPolling,
}

#[derive(Debug, Deserialize)]
struct StrictMailPolling {
    consecutive_failures: i32,
    last_poll_at: String,
    mailbox_key: String,
    next_poll_at: String,
    paused: bool,
    state_version: i64,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct StrictMailIntake {
    decision_code: String,
    external_mail_id: String,
    id: String,
    kind: String,
    mailbox_key: String,
    processed_at: String,
    received_at: String,
    status: String,
    world_id: String,
}

#[derive(Debug, Deserialize)]
struct StrictMailPagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
struct StrictMailIntakePage {
    intakes: Vec<StrictMailIntake>,
    pagination: StrictMailPagination,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictMailMetadata {
    added_count: i32,
    attachment_count: i32,
    auction_id: String,
    delivery_ids: Vec<String>,
    existing_count: i32,
    owner_result_code: String,
    preview_revision: i64,
    reason_code: String,
    submission_id: String,
    user_id: String,
    warning_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictMailIntakeDetail {
    decision_code: String,
    external_mail_id: String,
    id: String,
    kind: String,
    mailbox_key: String,
    metadata: StrictMailMetadata,
    processed_at: String,
    received_at: String,
    status: String,
    world_id: String,
}

#[derive(Debug, Deserialize)]
struct StrictMailTrigger {
    command_id: String,
    pollers: i64,
}

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
    let response = send_get(&format!("/api/v1/admin/mail?mailbox_key={encoded}")).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictMailState> =
        response.json().await.map_err(invalid_response_error)?;
    let polling = decode_mail_state(payload.data).map_err(invalid_response_error)?;
    if polling.mailbox_key != mailbox_key {
        return Err("invalid_response".to_owned());
    }
    Ok(polling)
}

pub(crate) async fn fetch_mail_intakes(
    query: AdminMailIntakeQuery,
    page: i64,
) -> Result<AdminMailIntakePage, String> {
    fetch_mail_intakes_with_size(query, page, TABLE_PAGE_SIZE).await
}

async fn fetch_mail_intakes_with_size(
    query: AdminMailIntakeQuery,
    page: i64,
    page_size: i64,
) -> Result<AdminMailIntakePage, String> {
    let mut url = format!("/api/v1/admin/mail/intake?page={page}&page_size={page_size}");
    append_query(&mut url, "mailbox_key", &query.mailbox_key);
    append_query(&mut url, "world_id", &query.world_id);
    append_query(&mut url, "kind", &query.kind);
    append_query(&mut url, "status", &query.status);
    append_query(&mut url, "decision", &query.decision);
    append_query(&mut url, "from", &normalize_datetime(&query.received_from));
    append_query(&mut url, "to", &normalize_datetime(&query.received_to));
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictMailIntakePage> =
        response.json().await.map_err(invalid_response_error)?;
    decode_mail_intake_page(payload.data, &query, page, page_size).map_err(invalid_response_error)
}

pub(crate) async fn fetch_mail_intake_detail(
    intake_id: String,
) -> Result<AdminMailIntakeDetail, String> {
    let response = send_get(&format!("/api/v1/admin/mail/intake/{intake_id}")).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictMailIntakeDetail> =
        response.json().await.map_err(invalid_response_error)?;
    let detail = decode_mail_intake_detail(payload.data).map_err(invalid_response_error)?;
    if detail.id != intake_id {
        return Err("invalid_response".to_owned());
    }
    Ok(detail)
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
) -> Result<AdminMailPolling, AdminApiFailure> {
    let csrf_token = super::fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let command = AdminMailPollingRequest {
        command_id,
        mailbox_key: polling.mailbox_key.clone(),
        paused,
        expected_state_version: polling.state_version,
    };
    let response = super::send_request(|abort_signal| {
        Request::put("/api/v1/admin/mail/polling")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<StrictMailState> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    let updated = decode_mail_state(payload.data)
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    if updated.mailbox_key != polling.mailbox_key
        || updated.paused != paused
        || Some(updated.state_version) != polling.state_version.checked_add(1)
    {
        return Err(AdminApiFailure::client("invalid_response"));
    }
    Ok(updated)
}

pub(crate) async fn trigger_mail_poll(command_id: Uuid) -> Result<i64, AdminApiFailure> {
    let csrf_token = super::fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let command = AdminMailPollTriggerRequest { command_id };
    let response = super::send_request(|abort_signal| {
        Request::post("/api/v1/admin/mail/polls:trigger")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: ApiResponse<StrictMailTrigger> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    if !valid_uuid(&payload.data.command_id)
        || payload.data.command_id != command_id.to_string()
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&payload.data.pollers)
    {
        return Err(AdminApiFailure::client("invalid_response"));
    }
    Ok(payload.data.pollers)
}

fn decode_mail_state(data: StrictMailState) -> Result<AdminMailPolling, &'static str> {
    let polling = data.polling;
    let last_poll_at = parse_admin_time(&polling.last_poll_at).ok_or("mail_poll_time")?;
    let next_poll_at = parse_admin_time(&polling.next_poll_at).ok_or("mail_next_time")?;
    if !valid_safe_text(&polling.mailbox_key, 256)
        || next_poll_at < last_poll_at
        || polling.consecutive_failures < 0
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&polling.state_version)
        || !valid_admin_time(&polling.updated_at)
    {
        return Err("mail_polling");
    }
    Ok(AdminMailPolling {
        mailbox_key: polling.mailbox_key,
        last_poll_at: polling.last_poll_at,
        next_poll_at: polling.next_poll_at,
        consecutive_failures: polling.consecutive_failures,
        paused: polling.paused,
        state_version: polling.state_version,
        updated_at: polling.updated_at,
    })
}

fn decode_mail_intake_page(
    data: StrictMailIntakePage,
    query: &AdminMailIntakeQuery,
    requested_page: i64,
    requested_page_size: i64,
) -> Result<AdminMailIntakePage, &'static str> {
    let pagination = data.pagination;
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&pagination.page)
        || pagination.page != requested_page
        || pagination.page_size != requested_page_size
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&pagination.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&pagination.total)
        || i64::try_from(data.intakes.len())
            .map_or(true, |count| count > pagination.page_size || count > pagination.total)
    {
        return Err("mail_pagination");
    }
    let intakes =
        data.intakes.into_iter().map(decode_mail_intake).collect::<Result<Vec<_>, _>>()?;
    let received_from = optional_query_time(&query.received_from)?;
    let received_to = optional_query_time(&query.received_to)?;
    if intakes.iter().any(|intake| {
        (!query.mailbox_key.is_empty() && intake.mailbox_key != query.mailbox_key)
            || (!query.world_id.is_empty() && intake.world_id != query.world_id)
            || (!query.kind.is_empty() && intake.kind != query.kind)
            || (!query.status.is_empty() && intake.status != query.status)
            || (!query.decision.is_empty() && intake.decision_code != query.decision)
            || parse_admin_time(&intake.received_at).is_none_or(|received| {
                received_from.is_some_and(|from| received < from)
                    || received_to.is_some_and(|to| received > to)
            })
    }) {
        return Err("mail_filter");
    }
    Ok(AdminMailIntakePage {
        intakes,
        pagination: AdminMailPaginationResponse {
            page: pagination.page,
            page_size: pagination.page_size,
            total: pagination.total,
        },
    })
}

fn decode_mail_intake(value: StrictMailIntake) -> Result<AdminMailIntake, &'static str> {
    let received_at = parse_admin_time(&value.received_at).ok_or("mail_received_time")?;
    let processed_at = parse_admin_time(&value.processed_at).ok_or("mail_processed_time")?;
    if !valid_uuid(&value.id)
        || !valid_safe_text(&value.mailbox_key, 256)
        || !valid_safe_text(&value.external_mail_id, 512)
        || !valid_safe_text(&value.world_id, 256)
        || !valid_trimmed_text(&value.kind)
        || !valid_trimmed_text(&value.status)
        || processed_at < received_at
    {
        return Err("mail_intake");
    }
    Ok(AdminMailIntake {
        id: value.id,
        mailbox_key: value.mailbox_key,
        external_mail_id: value.external_mail_id,
        world_id: value.world_id,
        kind: value.kind,
        status: value.status,
        decision_code: value.decision_code,
        received_at: value.received_at,
        processed_at: value.processed_at,
    })
}

fn decode_mail_intake_detail(
    value: StrictMailIntakeDetail,
) -> Result<AdminMailIntakeDetail, &'static str> {
    let summary = decode_mail_intake(StrictMailIntake {
        decision_code: value.decision_code,
        external_mail_id: value.external_mail_id,
        id: value.id,
        kind: value.kind,
        mailbox_key: value.mailbox_key,
        processed_at: value.processed_at,
        received_at: value.received_at,
        status: value.status,
        world_id: value.world_id,
    })?;
    let metadata = decode_mail_metadata(value.metadata)?;
    Ok(AdminMailIntakeDetail {
        id: summary.id,
        mailbox_key: summary.mailbox_key,
        external_mail_id: summary.external_mail_id,
        world_id: summary.world_id,
        kind: summary.kind,
        status: summary.status,
        decision_code: summary.decision_code,
        received_at: summary.received_at,
        processed_at: summary.processed_at,
        metadata,
    })
}

fn decode_mail_metadata(
    value: StrictMailMetadata,
) -> Result<AdminMailIntakeMetadataResponse, &'static str> {
    if value.added_count < 0
        || value.existing_count < 0
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.preview_revision)
        || value.attachment_count < 0
        || !valid_optional_metadata_text(&value.auction_id)
        || !valid_optional_metadata_text(&value.submission_id)
        || !valid_optional_metadata_text(&value.user_id)
        || !valid_optional_metadata_text(&value.owner_result_code)
        || !valid_optional_metadata_text(&value.reason_code)
        || !valid_metadata_list(&value.warning_codes)
        || !valid_metadata_list(&value.delivery_ids)
    {
        return Err("mail_metadata");
    }
    Ok(AdminMailIntakeMetadataResponse {
        auction_id: value.auction_id,
        submission_id: value.submission_id,
        user_id: value.user_id,
        added_count: value.added_count,
        existing_count: value.existing_count,
        preview_revision: value.preview_revision,
        attachment_count: value.attachment_count,
        owner_result_code: value.owner_result_code,
        reason_code: value.reason_code,
        warning_codes: value.warning_codes,
        delivery_ids: value.delivery_ids,
    })
}

fn optional_query_time(value: &str) -> Result<Option<f64>, &'static str> {
    if value.is_empty() {
        Ok(None)
    } else {
        parse_admin_time(&normalize_datetime(value)).map(Some).ok_or("mail_filter_time")
    }
}

fn valid_optional_metadata_text(value: &str) -> bool {
    value.is_empty() || valid_safe_text(value, 512)
}

fn valid_metadata_list(values: &[String]) -> bool {
    values.len() <= 1000 && values.iter().all(|value| valid_safe_text(value, 512))
}
