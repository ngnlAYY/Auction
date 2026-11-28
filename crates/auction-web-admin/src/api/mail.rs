//! Admin 邮件轮询与入站审查 HTTP 客户端。
//! 浏览器只读取轮询状态和入站安全摘要；暂停动作携带服务端快照中的 state version，
//! 立即轮询（`POST /api/v1/admin/mail/polls:trigger`）使用单个 command ID 唤醒全部 Worker
//! poller，浏览器不推导邮件处理状态。

use std::future::Future;

use auction_contracts::{
    AdminMailDisplayResponse, AdminMailEquipmentDiagnosticResponse,
    AdminMailIntakeMetadataResponse, AdminMailPollTriggerRequest, AdminMailPollTriggerResponse,
    AdminMailPollingRequest, ApiResponse,
};
pub(crate) use auction_contracts::{
    AdminMailIntakeDeliveryResponse as AdminMailIntakeDelivery,
    AdminMailIntakeDetailResponse as AdminMailIntakeDetail,
    AdminMailIntakeListResponse as AdminMailIntakePage,
    AdminMailIntakeSummaryResponse as AdminMailIntake,
    AdminMailPollingResponse as AdminMailPolling,
};

use gloo_net::http::Method;
use serde::Deserialize;
use uuid::Uuid;

use super::AdminWriteFailure;
use super::transport::WriteRequest;
use super::{
    MAX_JSON_SAFE_INTEGER, invalid_response_error, parse_admin_time, response_error, send_get,
    valid_admin_time, valid_safe_text, valid_trimmed_text, valid_uuid,
};

/// 轮询路径沿用允许扩展字段的旧边界，与 contracts 中闭集的 polling DTO 分开解码。
/// 列表、摘要、分页和触发结果的字段及扩展语义均与 contracts 一致，直接复用其 DTO。
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

const MAX_ADMIN_MAIL_RESPONSE_BYTES: usize = 1 << 20;

fn encode_query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// 入站列表的服务端筛选；空串表示该维度不筛选。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AdminMailIntakeQuery {
    pub world_id: String,
    pub kind: String,
    pub status: String,
}

impl AdminMailIntakeQuery {
    /// 入站邮件摘要与 URL 筛选共用的类型闭集。
    pub(crate) const KIND_VALUES: &[&str] =
        &["equipment", "bid", "delivery", "registration", "rebind", "rekey", "unknown"];
}

pub(crate) async fn fetch_mail_state(mailbox_key: String) -> Result<AdminMailPolling, String> {
    let encoded = encode_query(&mailbox_key);
    let response =
        send_get(&format!("/api/v1/admin/mail?mailbox_key={encoded}&include_diagnostics=true"))
            .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictMailState> = response
        .json_bounded(MAX_ADMIN_MAIL_RESPONSE_BYTES)
        .await
        .map_err(invalid_response_error)?;
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
    super::pagination::validate(page, page_size, 100, i64::from(i32::MAX))?;
    let mut url = format!("/api/v1/admin/mail/intake?page={page}&page_size={page_size}");
    append_query(&mut url, "world_id", &query.world_id);
    append_query(&mut url, "kind", &query.kind);
    append_query(&mut url, "status", &query.status);
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminMailIntakePage> = response
        .json_bounded(MAX_ADMIN_MAIL_RESPONSE_BYTES)
        .await
        .map_err(invalid_response_error)?;
    decode_mail_intake_page(payload.data, &query, page, page_size).map_err(invalid_response_error)
}

pub(crate) async fn fetch_mail_intake_detail(
    intake_id: String,
) -> Result<AdminMailIntakeDetail, String> {
    if !valid_uuid(&intake_id) {
        return Err("invalid_request".to_owned());
    }

    let response = send_get(&format!("/api/v1/admin/mail/intake/{intake_id}")).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminMailIntakeDetail> = response
        .json_bounded(MAX_ADMIN_MAIL_RESPONSE_BYTES)
        .await
        .map_err(invalid_response_error)?;
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

pub(crate) async fn set_mail_paused(
    command_id: Uuid,
    polling: AdminMailPolling,
    paused: bool,
) -> Result<AdminMailPolling, AdminWriteFailure> {
    let command = AdminMailPollingRequest {
        command_id,
        mailbox_key: polling.mailbox_key.clone(),
        paused,
        expected_state_version: polling.state_version,
    };
    let response = WriteRequest::general("/api/v1/admin/mail/polling", Method::PUT)
        .send_json(&command)
        .await?;
    let payload: ApiResponse<StrictMailState> = response
        .json_bounded(MAX_ADMIN_MAIL_RESPONSE_BYTES)
        .await
        .map_err(AdminWriteFailure::invalid_response)?;
    validate_mail_polling_receipt(payload, &command)
}

fn validate_mail_polling_receipt(
    payload: ApiResponse<StrictMailState>,
    command: &AdminMailPollingRequest,
) -> Result<AdminMailPolling, AdminWriteFailure> {
    let updated = decode_mail_state(payload.data).map_err(AdminWriteFailure::invalid_response)?;
    if updated.mailbox_key != command.mailbox_key
        || updated.paused != command.paused
        || Some(updated.state_version) != command.expected_state_version.checked_add(1)
    {
        return Err(AdminWriteFailure::outcome_unknown("invalid_response"));
    }
    Ok(updated)
}

pub(crate) async fn trigger_mail_poll(command_id: Uuid) -> Result<i64, AdminWriteFailure> {
    let command = AdminMailPollTriggerRequest { command_id };
    let response = WriteRequest::general("/api/v1/admin/mail/polls:trigger", Method::POST)
        .send_json(&command)
        .await?;
    poll_trigger_write_result(
        response.status(),
        response.json_bounded(MAX_ADMIN_MAIL_RESPONSE_BYTES),
        command_id,
    )
    .await
}

async fn poll_trigger_write_result(
    status: u16,
    body: impl Future<Output = Result<ApiResponse<AdminMailPollTriggerResponse>, gloo_net::Error>>,
    command_id: Uuid,
) -> Result<i64, AdminWriteFailure> {
    if status != 202 {
        return Err(AdminWriteFailure::invalid_response("mail_poll_trigger_status"));
    }
    let payload = body.await.map_err(AdminWriteFailure::invalid_response)?;
    accepted_pollers(payload, command_id)
        .ok_or_else(|| AdminWriteFailure::invalid_response("mail_poll_trigger_receipt"))
}

fn accepted_pollers(
    payload: ApiResponse<AdminMailPollTriggerResponse>,
    command_id: Uuid,
) -> Option<i64> {
    let receipt = payload.data;
    (payload.code == "accepted"
        && !payload.message.is_empty()
        && valid_uuid(&receipt.command_id)
        && receipt.command_id == command_id.to_string()
        && (1..=MAX_JSON_SAFE_INTEGER).contains(&receipt.pollers))
    .then_some(receipt.pollers)
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
        || polling.diagnostics.as_ref().is_some_and(|value| {
            !super::mail_diagnostics::valid_diagnostics(value, &polling.mailbox_key)
        })
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
    data: AdminMailIntakePage,
    query: &AdminMailIntakeQuery,
    requested_page: i64,
    requested_page_size: i64,
) -> Result<AdminMailIntakePage, &'static str> {
    let pagination = &data.pagination;
    if !super::pagination::valid_page_echo(
        requested_page,
        requested_page_size,
        pagination.page,
        pagination.page_size,
        pagination.total,
        data.intakes.len(),
    ) {
        return Err("mail_pagination");
    }
    if !data.intakes.iter().all(valid_mail_intake) {
        return Err("mail_intake");
    }
    if data.intakes.iter().any(|intake| {
        (!query.world_id.is_empty() && intake.world_id != query.world_id)
            || (!query.kind.is_empty() && intake.kind != query.kind)
            || (!query.status.is_empty() && intake.status != query.status)
    }) {
        return Err("mail_filter");
    }
    Ok(data)
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
        && valid_safe_text(&value.decision_code, 256)
        && AdminMailIntakeQuery::KIND_VALUES.contains(&value.kind.as_str())
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
    if value.deliveries.len() > value.metadata.delivery_ids.len() {
        return Err("mail_deliveries");
    }
    // 借用声明的 ID 并逐项消费，同时检查关联范围和重复项，避免每个任务重扫整份列表。
    let mut remaining = value
        .metadata
        .delivery_ids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    for delivery in &value.deliveries {
        if !valid_uuid(&delivery.id)
            || !remaining.remove(delivery.id.as_str())
            || !matches!(delivery.operation.as_str(), "send" | "return" | "take")
            || !valid_safe_text(&delivery.recipient, 512)
            || !super::valid_status(&delivery.status)
            || !(0..=MAX_JSON_SAFE_INTEGER).contains(&delivery.fencing_version)
            || delivery.attempt_count > i32::MAX as u32
            || !delivery.reason_code.as_deref().is_none_or(|value| valid_safe_text(value, 512))
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
        ("snapshot_invalid" | "filter_rejected", None) => false,
        (_, None) => true,
        _ => false,
    };
    let evidence_valid = match value.evidence_status.as_str() {
        "retained" | "body_unavailable" => {
            value.evidence_id.as_deref().is_some_and(valid_uuid) && value.retention_error.is_none()
        }
        "save_failed" | "completion_unconfirmed" => {
            value.evidence_id.as_deref().is_some_and(valid_uuid)
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
        || !valid_delivery_ids(&value.delivery_ids)
    {
        return Err("mail_metadata");
    }
    Ok(())
}

fn valid_optional_metadata_text(value: &str) -> bool {
    value.is_empty() || valid_safe_text(value, 512)
}

fn valid_metadata_list(values: &[String]) -> bool {
    values.len() <= 1000 && values.iter().all(|value| valid_safe_text(value, 512))
}

fn valid_delivery_ids(values: &[String]) -> bool {
    if values.len() > 1000 {
        return false;
    }
    let mut seen = std::collections::HashSet::with_capacity(values.len());
    values.iter().all(|value| valid_uuid(value) && seen.insert(value.as_str()))
}

#[cfg(test)]
mod tests {
    use std::future::{Future, ready};
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use auction_contracts::{AdminMailPollTriggerResponse, AdminMailPollingRequest, ApiResponse};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{
        AdminMailIntake, accepted_pollers, poll_trigger_write_result, valid_mail_intake,
        validate_mail_polling_receipt,
    };
    use crate::api::{AdminWriteFailure, Rejection};

    #[test]
    fn poll_trigger_requires_both_accepted_status_and_matching_command_id() {
        let command_id = Uuid::parse_str("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap();
        let other_id = Uuid::parse_str("0f8fad5b-d9cb-469f-a165-70867728950f").unwrap();
        let mut context = Context::from_waker(Waker::noop());
        for (status, receipt_id, accepted) in
            [(202, command_id, true), (200, command_id, false), (202, other_id, false)]
        {
            let body = format!(
                r#"{{"code":"accepted","message":"邮件轮询已触发","data":{{"command_id":"{receipt_id}","pollers":3}}}}"#
            );
            let decoded = serde_json::from_str(&body).map_err(gloo_net::Error::from);
            let mut reply = pin!(poll_trigger_write_result(status, ready(decoded), command_id));
            let Poll::Ready(result) = reply.as_mut().poll(&mut context) else {
                panic!("回执裁定应完成");
            };
            if accepted {
                assert!(result.is_ok());
            } else {
                let failure = result.expect_err("状态或命令 ID 错配时应保留未决");
                assert_eq!(failure.display_code(), "invalid_response");
                assert!(matches!(failure.classify(), Rejection::Unknown));
            }
        }
    }

    #[test]
    fn mail_polling_receipt_must_match_mailbox_next_version_and_target_state() {
        let mut command = AdminMailPollingRequest {
            command_id: Uuid::new_v4(),
            mailbox_key: "mail-a".to_owned(),
            paused: true,
            expected_state_version: 1,
        };
        let mut wire: Value = serde_json::from_str(include_str!(
            "../../../auction-contracts/tests/fixtures/admin/mail.json"
        ))
        .unwrap();
        wire["data"]["polling"]["mailbox_key"] = json!("mail-a");
        wire["data"]["polling"]["state_version"] = json!(2);
        wire["data"]["polling"]["paused"] = json!(true);
        let matching =
            validate_mail_polling_receipt(serde_json::from_value(wire.clone()).unwrap(), &command)
                .unwrap();
        assert_eq!(matching.mailbox_key, "mail-a");
        assert_eq!(matching.state_version, 2);
        assert!(matching.paused);

        for (field, wrong) in [
            ("mailbox_key", json!("mail-b")),
            ("state_version", json!(3)),
            ("paused", json!(false)),
        ] {
            let mut mismatched = wire.clone();
            mismatched["data"]["polling"][field] = wrong;
            assert_eq!(
                validate_mail_polling_receipt(
                    serde_json::from_value(mismatched).unwrap(),
                    &command
                ),
                Err(AdminWriteFailure::outcome_unknown("invalid_response")),
                "{field}",
            );
        }

        command.paused = false;
        wire["data"]["polling"]["paused"] = json!(false);
        assert!(
            !validate_mail_polling_receipt(serde_json::from_value(wire).unwrap(), &command)
                .unwrap()
                .paused
        );
    }

    #[test]
    fn poll_trigger_uses_the_count_from_the_accepted_receipt() {
        let command_id = Uuid::parse_str("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap();
        let mut payload = ApiResponse::accepted(
            "邮件轮询已触发",
            AdminMailPollTriggerResponse { command_id: command_id.to_string(), pollers: 3 },
        );
        assert_eq!(accepted_pollers(payload.clone(), command_id), Some(3));
        assert_eq!(accepted_pollers(payload.clone(), Uuid::nil()), None);
        payload.code = "ok".to_owned();
        assert_eq!(accepted_pollers(payload.clone(), command_id), None);
        payload.code = "accepted".to_owned();
        payload.data.pollers = 0;
        assert_eq!(accepted_pollers(payload, command_id), None);
    }

    #[test]
    fn mail_intake_accepts_rekey_and_rejects_unrecognized_kind() {
        let mut intake: AdminMailIntake = serde_json::from_value(json!({
            "id": "0f8fad5b-d9cb-469f-a165-70867728950e",
            "mailbox_key": "main-mailbox",
            "external_mail_id": "1234",
            "world_id": "main",
            "kind": "rekey",
            "status": "accepted",
            "decision_code": "rekeyed",
            "received_at": "2026-11-06T10:00:00Z",
            "processed_at": "2026-11-06T10:00:01Z",
            "has_message_display": false
        }))
        .unwrap();
        assert!(valid_mail_intake(&intake));
        intake.kind = "other_kind".to_owned();
        assert!(!valid_mail_intake(&intake));
    }

    #[test]
    fn rekey_reads_decode_backend_contract_fixtures() {
        use auction_contracts::ApiResponse;

        use super::{
            AdminMailIntakeDetail, AdminMailIntakePage, AdminMailIntakeQuery,
            decode_mail_intake_detail, decode_mail_intake_page,
        };

        for (body, kind, count) in [
            (
                include_str!("../../../auction-contracts/tests/fixtures/admin/mail-intake.json"),
                "",
                2,
            ),
            (
                include_str!(
                    "../../../auction-contracts/tests/fixtures/admin/mail-intake-rekey.json"
                ),
                "rekey",
                1,
            ),
        ] {
            let payload: ApiResponse<AdminMailIntakePage> =
                serde_json::from_str(body).expect("收件样本");
            let query = AdminMailIntakeQuery { kind: kind.to_owned(), ..Default::default() };
            let page = decode_mail_intake_page(payload.data, &query, 1, 50).expect("入站邮件页");
            assert_eq!(page.intakes.len(), count);
            assert_eq!(page.intakes[0].kind, "rekey");
        }
        let payload: ApiResponse<AdminMailIntakeDetail> = serde_json::from_str(include_str!(
            "../../../auction-contracts/tests/fixtures/admin/mail-intake-detail.json"
        ))
        .expect("收件详情样本");
        let detail = decode_mail_intake_detail(payload.data).expect("rekey 详情");
        assert_eq!(detail.kind, "rekey");
        assert!(detail.subject.is_none());
    }

    #[test]
    fn legacy_mail_intake_page_decodes_projected_text_at_the_byte_limit() {
        use super::{AdminMailIntakePage, AdminMailIntakeQuery, decode_mail_intake_page};
        use crate::api::contract_fixtures::admin_fixture;

        let payload: ApiResponse<AdminMailIntakePage> =
            serde_json::from_str(admin_fixture!("mail-intake-legacy")).expect("历史入站邮件样本");
        let query = AdminMailIntakeQuery::default();
        let mut page =
            decode_mail_intake_page(payload.data, &query, 1, 20).expect("历史入站邮件页");
        assert_eq!(page.intakes.len(), 9);

        let subject = page
            .intakes
            .iter_mut()
            .find(|row| row.external_mail_id == "9026")
            .and_then(|row| row.subject.as_mut())
            .expect("边界主题");
        assert_eq!(subject.len(), 512);
        assert!(subject.ends_with(" login key[REDACTED]"));
        subject.push('x');
        assert_eq!(decode_mail_intake_page(page, &query, 1, 20), Err("mail_intake"));
    }

    #[test]
    fn legacy_mail_intake_detail_decodes_projected_text_and_rejects_control_text() {
        use super::{AdminMailIntakeDetail, decode_mail_intake_detail};
        use crate::api::contract_fixtures::admin_fixture;

        let fixtures: [ApiResponse<AdminMailIntakeDetail>; 4] =
            serde_json::from_str(admin_fixture!("mail-intake-legacy-detail"))
                .expect("四种历史入站邮件详情样本");
        let [all_redacted, partial, at_limit, trimmed_sender] =
            fixtures.map(|payload| decode_mail_intake_detail(payload.data).expect("历史邮件详情"));
        assert_eq!(
            [
                all_redacted.mailbox_key.as_str(),
                all_redacted.external_mail_id.as_str(),
                all_redacted.world_id.as_str(),
                all_redacted.decision_code.as_str(),
                all_redacted.sender.as_deref().unwrap(),
                all_redacted.subject.as_deref().unwrap(),
            ],
            ["[REDACTED]"; 6]
        );
        assert_eq!(partial.subject.as_deref(), Some("补充说明：登录 key[REDACTED]"));
        let subject = at_limit.subject.as_deref().expect("边界主题");
        assert_eq!(subject.len(), 512);
        assert!(subject.ends_with(" login key[REDACTED]"));
        assert_eq!(trimmed_sender.sender.as_deref(), Some("Player"));
        assert_eq!(trimmed_sender.subject.as_deref(), Some("拍卖问题"));

        let mut invalid = all_redacted.clone();
        invalid.subject = Some("多行\n主题".to_owned());
        assert_eq!(decode_mail_intake_detail(invalid), Err("mail_intake"));

        let mut invalid = all_redacted;
        invalid.external_mail_id = "9001\t9002".to_owned();
        assert_eq!(decode_mail_intake_detail(invalid), Err("mail_intake"));
    }

    #[test]
    fn mail_poll_contract_fixtures() {
        use super::{ApiResponse, StrictMailState, decode_mail_state};
        use crate::api::contract_fixtures::{admin_fixture, decode};

        for (body, expected_diagnostics) in
            [(admin_fixture!("mail"), false), (admin_fixture!("mail-diagnostics"), true)]
        {
            let payload: ApiResponse<StrictMailState> = decode(body, "/data/polling", "paused");
            let polling = decode_mail_state(payload.data).expect("轮询响应");
            assert_eq!(polling.mailbox_key, "main");
            assert!(!polling.paused);
            assert_eq!(polling.state_version, 3);
            assert_eq!(polling.diagnostics.is_some(), expected_diagnostics);
            if let Some(diagnostics) = polling.diagnostics {
                assert_eq!(diagnostics.completed_external_mail_id.as_deref(), Some("9002"));
                assert!(diagnostics.progress.is_none());
            }
        }
    }
}
