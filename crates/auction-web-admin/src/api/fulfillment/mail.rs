//! Admin 邮件轮询与入站审查 HTTP 客户端。
//! 浏览器只读取轮询状态和入站安全摘要；暂停动作携带服务端快照中的 state version，
//! A20 触发动作使用单个 command ID 唤醒全部 Worker poller，浏览器不推导邮件处理状态。

use auction_contracts::{
    AdminMailDisplayResponse, AdminMailEquipmentDiagnosticResponse,
    AdminMailIntakeMetadataResponse, AdminMailPaginationResponse, AdminMailPollTriggerRequest,
    AdminMailPollingRequest,
};
pub(crate) use auction_contracts::{
    AdminMailIntakeDeliveryResponse as AdminMailIntakeDelivery,
    AdminMailIntakeDetailResponse as AdminMailIntakeDetail,
    AdminMailIntakeListResponse as AdminMailIntakePage,
    AdminMailIntakeSummaryResponse as AdminMailIntake,
    AdminMailPollingResponse as AdminMailPolling,
};
#[path = "mail/diagnostics.rs"]
mod diagnostics;

use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    AdminApiFailure, ApiResponse, MAX_JSON_SAFE_INTEGER, invalid_request_error,
    invalid_response_error, parse_admin_time, response_error, response_failure, send_get,
    valid_admin_time, valid_safe_text, valid_trimmed_text, valid_uuid,
};

/// 旧 Mail decoder 允许非破坏性扩展字段；所有旧字段及新详情的固定 metadata 仍逐项校验。
/// 保留镜像的理由：contracts 的 `AdminMailStateResponse`/`AdminMailIntakeSummaryResponse`/
/// `AdminMailIntakeListResponse`/`AdminMailPollTriggerResponse` 对应同一 wire，但旧契约允许
/// 服务端增加扩展字段，这些 DTO 不能加 `deny_unknown_fields`；镜像继续要求旧字段显式存在。
/// 同步义务：上述 DTO 字段集变化时必须同步本镜像；下方构造 DTO 的穷举字段列表会在编译期
/// 捕捉漂移。详情与 metadata 的 wire 是闭集，已由 contracts 的 `AdminMailIntakeDetailResponse`
/// 与 `AdminMailIntakeMetadataResponse` 承担，不再保留本地镜像。
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
    #[serde(default)]
    diagnostics: Option<auction_contracts::AdminMailPollingDiagnosticsResponse>,
}

#[derive(Debug, Deserialize)]
struct StrictMailIntake {
    decision_code: String,
    external_mail_id: String,
    has_message_display: bool,
    id: String,
    kind: String,
    mailbox_key: String,
    processed_at: String,
    received_at: String,
    sender: Option<String>,
    status: String,
    subject: Option<String>,
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
    let response =
        send_get(&format!("/api/v1/admin/mail?mailbox_key={encoded}&include_diagnostics=true"))
            .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictMailState> =
        response.json().await.map_err(invalid_response_error)?;
    let polling = decode_mail_state(payload.data).map_err(invalid_response_error)?;
    if polling.mailbox_key != mailbox_key || polling.diagnostics.is_none() {
        return Err("invalid_response".to_owned());
    }
    Ok(polling)
}

pub(crate) async fn fetch_mail_intakes(
    query: AdminMailIntakeQuery,
    page: i64,
    page_size: i64,
) -> Result<AdminMailIntakePage, String> {
    fetch_mail_intakes_with_size(query, page, page_size).await
}

async fn fetch_mail_intakes_with_size(
    query: AdminMailIntakeQuery,
    page: i64,
    page_size: i64,
) -> Result<AdminMailIntakePage, String> {
    super::pagination::validate(page, page_size, 100, i64::from(i32::MAX))?;
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
    let payload: ApiResponse<AdminMailIntakeDetail> =
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
        || polling
            .diagnostics
            .as_ref()
            .is_some_and(|value| !diagnostics::valid_diagnostics(value, &polling.mailbox_key))
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
        diagnostics: polling.diagnostics,
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
    let intake = AdminMailIntake {
        id: value.id,
        mailbox_key: value.mailbox_key,
        external_mail_id: value.external_mail_id,
        has_message_display: value.has_message_display,
        world_id: value.world_id,
        kind: value.kind,
        status: value.status,
        decision_code: value.decision_code,
        received_at: value.received_at,
        processed_at: value.processed_at,
        sender: value.sender,
        subject: value.subject,
    };
    valid_mail_intake(&intake).then_some(intake).ok_or("mail_intake")
}

/// 摘要字段的共享值域谓词；详情路径复用同一校验，保证两个端点的旧字段语义一致。
fn valid_mail_intake(value: &AdminMailIntake) -> bool {
    let Some(received_at) = parse_admin_time(&value.received_at) else {
        return false;
    };
    let Some(processed_at) = parse_admin_time(&value.processed_at) else {
        return false;
    };
    valid_uuid(&value.id)
        && valid_safe_text(&value.mailbox_key, 256)
        && valid_safe_text(&value.external_mail_id, 512)
        && valid_safe_text(&value.world_id, 256)
        && value.sender.as_deref().is_none_or(|sender| valid_safe_text(sender, 256))
        && value.subject.as_deref().is_none_or(|subject| valid_safe_text(subject, 512))
        && valid_trimmed_text(&value.kind)
        && valid_trimmed_text(&value.status)
        && processed_at >= received_at
}

fn decode_mail_intake_detail(
    value: AdminMailIntakeDetail,
) -> Result<AdminMailIntakeDetail, &'static str> {
    let summary = AdminMailIntake {
        id: value.id.clone(),
        mailbox_key: value.mailbox_key.clone(),
        external_mail_id: value.external_mail_id.clone(),
        has_message_display: value.message_display.is_some(),
        world_id: value.world_id.clone(),
        kind: value.kind.clone(),
        status: value.status.clone(),
        decision_code: value.decision_code.clone(),
        received_at: value.received_at.clone(),
        processed_at: value.processed_at.clone(),
        sender: value.sender.clone(),
        subject: value.subject.clone(),
    };
    if !valid_mail_intake(&summary) {
        return Err("mail_intake");
    }
    decode_mail_metadata(&value.metadata)?;
    decode_mail_deliveries(&value)?;
    decode_mail_display(value.message_display.as_ref())?;
    decode_mail_equipment_diagnostic(value.equipment_diagnostic.as_ref())?;
    Ok(value)
}

/// 关联任务只接受本封邮件列出的唯一 delivery；重试仍由服务端状态版本最终裁决。
fn decode_mail_deliveries(value: &AdminMailIntakeDetail) -> Result<(), &'static str> {
    let mut seen = std::collections::HashSet::new();
    if value.deliveries.len() > value.metadata.delivery_ids.len() {
        return Err("mail_deliveries");
    }
    for delivery in &value.deliveries {
        if !valid_uuid(&delivery.id)
            || !seen.insert(&delivery.id)
            || !value.metadata.delivery_ids.contains(&delivery.id)
            || !matches!(delivery.operation.as_str(), "send" | "return" | "take")
            || !valid_safe_text(&delivery.recipient, 512)
            || !super::valid_status(&delivery.status)
            || !(0..=MAX_JSON_SAFE_INTEGER).contains(&delivery.fencing_version)
            || (matches!(delivery.status.as_str(), "failed" | "manual_reconcile")
                && delivery.fencing_version == 0)
            || !delivery.failure_code.as_deref().is_none_or(|value| valid_safe_text(value, 512))
            || !delivery
                .external_reference
                .as_deref()
                .is_none_or(|value| valid_safe_text(value, 512))
        {
            return Err("mail_deliveries");
        }
    }
    Ok(())
}

fn decode_mail_equipment_diagnostic(
    value: Option<&AdminMailEquipmentDiagnosticResponse>,
) -> Result<(), &'static str> {
    let Some(value) = value else {
        return Ok(());
    };
    let detail_valid = match (value.category.as_str(), value.detail_code.as_deref()) {
        ("protocol_drift", Some(detail)) if value.stage == "parse" => matches!(
            detail,
            "equipment_name_invalid"
                | "equipment_quality_unknown"
                | "equipment_kind_unknown"
                | "equipment_category_unknown"
                | "equipment_category_mismatch"
                | "equipment_level_invalid"
                | "equipment_page_structure_invalid"
                | "equipment_attributes_invalid"
                | "equipment_canonical_mismatch"
        ),
        ("snapshot_invalid", Some(detail)) => matches!(
            detail,
            "adapter_invalid"
                | "fingerprint_invalid"
                | "equipment_id_invalid"
                | "name_invalid"
                | "category_invalid"
                | "level_invalid"
                | "collected_at_invalid"
                | "collected_at_future"
                | "attributes_invalid"
        ),
        ("filter_rejected", Some(detail)) => matches!(
            detail,
            "quality_not_allowed"
                | "prefix_not_allowed"
                | "suffix_not_allowed"
                | "category_not_allowed"
                | "equipment_type_not_allowed"
                | "level_not_allowed"
                | "attributes_invalid"
                | "tradeable_required"
        ),
        (_, None) => true,
        _ => false,
    };
    let evidence_valid = match value.evidence_status.as_str() {
        "retained" | "body_unavailable" => {
            value.evidence_id.as_deref().is_some_and(valid_evidence_id)
                && value.retention_error.is_none()
        }
        "save_failed" | "completion_unconfirmed" => {
            value.evidence_id.as_deref().is_some_and(valid_evidence_id)
                && value.retention_error.as_deref().is_some_and(valid_machine_code)
        }
        "prepare_failed" => {
            value.evidence_id.is_none()
                && value.retention_error.as_deref().is_some_and(valid_machine_code)
        }
        "disabled" => value.evidence_id.is_none() && value.retention_error.is_none(),
        _ => false,
    };
    if value.schema_version != 1
        || !valid_safe_text(&value.equipment_id, 64)
        || !value.equipment_id.bytes().all(|byte| byte.is_ascii_digit())
        || !(1..=1000).contains(&value.attachment_position)
        || !matches!(value.stage.as_str(), "request" | "response" | "parse" | "validate")
        || !matches!(
            value.category.as_str(),
            "configuration"
                | "credential_unavailable"
                | "session_expired"
                | "timeout"
                | "network"
                | "redirect"
                | "response_too_large"
                | "content_encoding"
                | "rate_limited"
                | "server_error"
                | "http_status"
                | "content_type"
                | "protocol_drift"
                | "identity_mismatch"
                | "snapshot_invalid"
                | "filter_rejected"
        )
        || !detail_valid
        || value.http_status.is_some_and(|status| !(100..=599).contains(&status))
        || value.elapsed_millis.is_some_and(|elapsed| elapsed > 300_000)
        || !evidence_valid
    {
        return Err("mail_equipment_diagnostic");
    }
    Ok(())
}

fn valid_evidence_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil())
}

fn valid_machine_code(value: &str) -> bool {
    matches!(
        value,
        "invalid_context"
            | "invalid_outcome"
            | "invalid_message"
            | "payload_too_large"
            | "body_unavailable"
            | "encryption"
            | "serialization"
            | "io"
            | "capacity"
            | "not_found"
            | "authentication"
            | "unsupported_platform"
            | "retention_timeout"
            | "retention_cancelled"
            | "recorder_unavailable"
    )
}

fn decode_mail_display(value: Option<&AdminMailDisplayResponse>) -> Result<(), &'static str> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.schema_version != 1
        || value.redaction_version != 1
        || value.body.len() > 64 * 1024
        || value
            .body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        || value.attachment_ids.len() > 1000
        || value.attachment_ids.iter().enumerate().any(|(index, id)| {
            !valid_safe_text(id, 128) || (index > 0 && value.attachment_ids[index - 1] >= *id)
        })
        || value.credits < 0
        || value.cod < 0
    {
        return Err("mail_display");
    }
    Ok(())
}

fn decode_mail_metadata(value: &AdminMailIntakeMetadataResponse) -> Result<(), &'static str> {
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
    Ok(())
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
