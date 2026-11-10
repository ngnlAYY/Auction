//! Admin 投递详情与人工 Sent 证明的同源 HTTP 客户端。
//!
//! 只消费 `auction-contracts` 的安全投影；详情与预览都是只读，确认复用服务端稳定命令 ID
//! 与幂等摘要。浏览器不推导状态机，也不放宽服务端闭集：未知操作类型、越界数字、非规范
//! 摘要或错误投递身份一律 fail-closed。

use auction_contracts::{
    AdminDeliveryAcknowledgeRequest, AdminDeliveryAcknowledgeResponse, AdminDeliveryDetailResponse,
    AdminDeliverySentEvidenceRequest, AdminDeliverySentProofConfirmRequest,
    AdminDeliverySentProofPreviewRequest, AdminDeliverySentProofPreviewResponse,
    AdminDeliverySentProofResponse,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    AdminApiFailure, MAX_JSON_SAFE_INTEGER, PlayerSupportBundle, fetch_csrf_token,
    invalid_request_error, invalid_response_error, response_error, response_failure, send_get,
    send_request, valid_safe_text, valid_success_code, valid_uuid,
};

mod decode;
mod queue;

use self::decode::{StrictDeliveryEnvelope, decode_delivery, parse_delivery_time};
pub(crate) use decode::valid_status;
pub(crate) use queue::{DeliveryAction, DeliverySearchFilters, execute_delivery, fetch_deliveries};

/// 与服务端 `MAX_PLAYER_SUPPORT_ROWS` 对齐；超过即视为损坏响应。
const MAX_PLAYER_SUPPORT_ROWS: usize = 50;

/// 读取单条投递的完整证据详情；响应必须精确对应请求的投递 ID。
pub(crate) async fn fetch_delivery_detail(
    delivery_id: String,
) -> Result<AdminDeliveryDetailResponse, String> {
    if !valid_uuid(&delivery_id) {
        return Err("invalid_delivery_id".to_owned());
    }
    let response = send_get(&format!("/api/v1/admin/deliveries/{delivery_id}")).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: StrictDeliveryEnvelope<AdminDeliveryDetailResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let detail = decode_detail(payload).map_err(|_| invalid_response_error("delivery_detail"))?;
    if detail.delivery.id != delivery_id {
        return Err(invalid_response_error("delivery_detail_id"));
    }
    Ok(detail)
}

/// 无副作用预览；返回闭集 blockers，确认前必须由服务端再次判定。
pub(crate) async fn preview_delivery_sent_proof(
    delivery_id: String,
    evidence_reference: String,
    evidence: AdminDeliverySentEvidenceRequest,
) -> Result<AdminDeliverySentProofPreviewResponse, AdminApiFailure> {
    if !valid_uuid(&delivery_id) || !valid_safe_text(&evidence_reference, 128) {
        return Err(AdminApiFailure::client("invalid_delivery_command"));
    }
    let url = format!("/api/v1/admin/deliveries/{delivery_id}/sent-proof/preview");
    let request = AdminDeliverySentProofPreviewRequest { evidence_reference, evidence };
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&request)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: StrictDeliveryEnvelope<AdminDeliverySentProofPreviewResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_preview(payload, &delivery_id)
        .map_err(|_| AdminApiFailure::client(invalid_response_error("delivery_sent_preview")))
}

/// 确认无资金/资产影响的历史失败已处置；不改变投递状态，只追加解除告警的事实。
pub(crate) async fn acknowledge_delivery(
    command_id: Uuid,
    delivery_id: String,
    expected_fencing_version: i64,
    reason: String,
) -> Result<AdminDeliveryAcknowledgeResponse, AdminApiFailure> {
    if command_id.is_nil()
        || !valid_uuid(&delivery_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&expected_fencing_version)
        || !valid_safe_text(&reason, 2048)
    {
        return Err(AdminApiFailure::client("invalid_delivery_command"));
    }
    let request = AdminDeliveryAcknowledgeRequest { command_id, expected_fencing_version, reason };
    let url = format!("/api/v1/admin/deliveries/{delivery_id}/acknowledge");
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&request)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: StrictDeliveryEnvelope<AdminDeliveryAcknowledgeResponse> =
        response
            .json()
            .await
            .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    if !valid_success_code(&payload.code)
        || payload.message.is_empty()
        || payload.data.delivery_id != delivery_id
        || payload.data.fencing_version != expected_fencing_version
        || payload.data.kind != "operator_acknowledged"
    {
        return Err(AdminApiFailure::client(invalid_response_error("delivery_acknowledge")));
    }
    Ok(payload.data)
}

/// 人工 Sent 证明确认的完整提交；`command_id` 由页面命令宿主生成并在重试间保持。
pub(crate) struct SentProofCommand {
    pub(crate) command_id: Uuid,
    pub(crate) delivery_id: String,
    pub(crate) expected_fencing_version: i64,
    pub(crate) reason: String,
    pub(crate) evidence_reference: String,
    pub(crate) evidence: AdminDeliverySentEvidenceRequest,
    pub(crate) accept_collected_assets: bool,
    pub(crate) accept_attempt_history: bool,
}

/// 提交人工 Sent 证明确认；同 `command_id` 同参由服务端重放原结果。
pub(crate) async fn confirm_delivery_sent_proof(
    command: SentProofCommand,
) -> Result<AdminDeliverySentProofResponse, AdminApiFailure> {
    let SentProofCommand {
        command_id,
        delivery_id,
        expected_fencing_version,
        reason,
        evidence_reference,
        evidence,
        accept_collected_assets,
        accept_attempt_history,
    } = command;
    if command_id.is_nil()
        || !valid_uuid(&delivery_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&expected_fencing_version)
        || !valid_safe_text(&evidence_reference, 128)
        || !valid_safe_text(&reason, 2048)
    {
        return Err(AdminApiFailure::client("invalid_delivery_command"));
    }
    let url = format!("/api/v1/admin/deliveries/{delivery_id}/sent-proof/confirm");
    let request = AdminDeliverySentProofConfirmRequest {
        command_id,
        expected_fencing_version,
        reason,
        evidence_reference,
        evidence,
        accept_collected_assets,
        accept_attempt_history,
    };
    let csrf_token = fetch_csrf_token().await.map_err(AdminApiFailure::client)?;
    let response = send_request(|abort_signal| {
        Request::post(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&request)
            .map_err(invalid_request_error)
    })
    .await
    .map_err(AdminApiFailure::client)?;
    if !response.ok() {
        return Err(response_failure(response).await);
    }
    let payload: StrictDeliveryEnvelope<AdminDeliverySentProofResponse> = response
        .json()
        .await
        .map_err(|error| AdminApiFailure::client(invalid_response_error(error)))?;
    decode_confirmation(payload, command_id, &delivery_id)
        .map_err(|_| AdminApiFailure::client(invalid_response_error("delivery_sent_confirmation")))
}

fn decode_detail(
    payload: StrictDeliveryEnvelope<AdminDeliveryDetailResponse>,
) -> Result<AdminDeliveryDetailResponse, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }
    let data = payload.data;
    let _delivery = decode_delivery(data.delivery.clone())?;
    if data.attempts.iter().any(|attempt| !valid_attempt(attempt))
        || data.evidence.iter().any(|evidence| !valid_evidence(evidence))
        || !valid_custody(&data.custody)
        || data.dispositions.iter().any(|item| !valid_disposition(item))
        || data.obligation.as_ref().is_some_and(|item| !valid_obligation(item))
        || data.recovery.as_ref().is_some_and(|item| !valid_recovery(item))
    {
        return Err(());
    }
    Ok(data)
}

fn decode_preview(
    payload: StrictDeliveryEnvelope<AdminDeliverySentProofPreviewResponse>,
    delivery_id: &str,
) -> Result<AdminDeliverySentProofPreviewResponse, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }
    let preview = payload.data;
    // 只接受本次请求的投递身份与正版本；blockers 是服务端闭集，不能由浏览器补全。
    if preview.delivery_id != delivery_id
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&preview.expected_fencing_version)
        || !valid_stored_text(&preview.external_reference)
        || preview.blockers.len() > 32
        || preview.blockers.iter().any(|blocker| !valid_safe_text(blocker, 96))
        || (preview.eligible && !preview.blockers.is_empty())
    {
        return Err(());
    }
    Ok(preview)
}

fn decode_confirmation(
    payload: StrictDeliveryEnvelope<AdminDeliverySentProofResponse>,
    command_id: Uuid,
    delivery_id: &str,
) -> Result<AdminDeliverySentProofResponse, ()> {
    if !valid_success_code(&payload.code) || payload.message.is_empty() {
        return Err(());
    }
    let result = payload.data;
    if result.command_id != command_id.to_string()
        || result.delivery_id != delivery_id
        || !valid_stored_text(&result.external_reference)
    {
        return Err(());
    }
    Ok(result)
}

fn valid_attempt(value: &auction_contracts::AdminDeliveryAttemptResponse) -> bool {
    if !matches!(value.operation.as_str(), "send" | "take")
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.fencing_version)
        || !valid_stored_text(&value.world_id)
        || parse_delivery_time(&value.sending_at).is_none()
        || !value.recipient.as_deref().is_none_or(valid_stored_text)
        || !value.subject.as_deref().is_none_or(|text| valid_safe_text(text, 512))
        || !value.body_digest.as_deref().is_none_or(valid_hex_digest)
        || !value.credits.is_none_or(valid_amount)
        || !value.cod.is_none_or(valid_amount)
        || !value.equipment_ids.iter().all(|id| valid_positive_decimal(id))
        || !value.external_mail_id.as_deref().is_none_or(valid_mail_id)
        || !value.bound_sent_mail_id.as_deref().is_none_or(valid_mail_id)
        || !value.credential.as_ref().is_none_or(|credential| {
            valid_uuid(&credential.credential_id)
                && (0..=MAX_JSON_SAFE_INTEGER).contains(&credential.credential_version)
                && (0..=MAX_JSON_SAFE_INTEGER).contains(&credential.runtime_generation)
        })
    {
        return false;
    }
    match value.operation.as_str() {
        "send" => {
            value.recipient.is_some()
                && value.subject.is_some()
                && value.credits.is_some()
                && value.cod.is_some()
        }
        _ => value.external_mail_id.is_some(),
    }
}

fn valid_evidence(value: &auction_contracts::AdminDeliveryEvidenceResponse) -> bool {
    valid_safe_text(&value.reference, 128)
        && matches!(
            value.kind.as_str(),
            "mailbox_sweep"
                | "sent_reference_sighting"
                | "compose_discard"
                | "sent_operator_confirmed"
        )
        && value.external_mail_id.as_deref().is_none_or(valid_mail_id)
        && value.observed_at.as_deref().is_none_or(|time| parse_delivery_time(time).is_some())
        && value.sent_at.as_deref().is_none_or(|time| parse_delivery_time(time).is_some())
        && value.recipient.as_deref().is_none_or(|text| valid_safe_text(text, 512))
        && value.subject.as_deref().is_none_or(|text| valid_safe_text(text, 512))
        && value.body_digest.as_deref().is_none_or(valid_hex_digest)
        && value.cod_pending.is_none_or(valid_amount)
        && value.remaining_credits.is_none_or(valid_amount)
        && value.remaining_equipment_count.is_none_or(valid_amount)
        && value.receipt.as_deref().is_none_or(|text| text.len() <= 2048)
}

fn valid_custody(value: &auction_contracts::AdminDeliveryCustodyResponse) -> bool {
    value.cod_pending.is_none_or(valid_amount)
        && value.remaining_credits.is_none_or(valid_amount)
        && value.receipt.as_deref().is_none_or(|text| text.len() <= 2048)
        && value.observed_at.as_deref().is_none_or(|time| parse_delivery_time(time).is_some())
}

fn valid_disposition(value: &auction_contracts::AdminDeliveryDispositionResponse) -> bool {
    valid_uuid(&value.command_id)
        && matches!(
            value.kind.as_str(),
            "release"
                | "retry_failed"
                | "reconcile"
                | "sent_operator_confirmed"
                | "operator_acknowledged"
                | "auto_sent_proof"
                | "operator_sent_confirmation"
                | "admin_failed"
                | "equipment_loss"
                | "cod_returned"
                | "legacy_self"
                | "preflight_retired"
        )
        && parse_delivery_time(&value.occurred_at).is_some()
        && value.status.as_deref().is_none_or(valid_stored_text)
        && value
            .fencing_version
            .is_none_or(|version| (0..=MAX_JSON_SAFE_INTEGER).contains(&version))
        && value.failure_code.as_deref().is_none_or(valid_stored_text)
        && value.external_reference.as_deref().is_none_or(valid_stored_text)
        && value.actor.as_deref().is_none_or(|text| valid_safe_text(text, 128))
        && value.reason.as_deref().is_none_or(|text| text.len() <= 2048)
}

fn valid_obligation(value: &auction_contracts::AdminDeliveryObligationResponse) -> bool {
    valid_uuid(&value.obligation_id)
        && valid_uuid(&value.auction_id)
        && valid_uuid(&value.lot_id)
        && valid_stored_text(&value.world_id)
        && valid_stored_text(&value.obligation_kind)
        && valid_stored_text(&value.business_state)
        && valid_amount(value.amount)
        && value.fulfillment_state.as_deref().is_none_or(valid_stored_text)
        && value.fulfillment_error_code.as_deref().is_none_or(valid_stored_text)
}

fn valid_recovery(value: &auction_contracts::AdminDeliveryRecoveryResponse) -> bool {
    valid_uuid(&value.recovery_id)
        && valid_uuid(&value.payment_obligation_id)
        && valid_stored_text(&value.stage)
        && parse_delivery_time(&value.payment_deadline_at).is_some()
        && value.resolution.as_deref().is_none_or(valid_stored_text)
        && value.blocking_code.as_deref().is_none_or(valid_stored_text)
        && value.manual_reconcile_delivery_id.as_deref().is_none_or(valid_uuid)
}

fn valid_stored_text(value: &str) -> bool {
    !value.is_empty() && value == value.trim()
}

fn valid_amount(value: i64) -> bool {
    (0..=MAX_JSON_SAFE_INTEGER).contains(&value)
}

fn valid_positive_decimal(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && !value.starts_with('0')
}

fn valid_mail_id(value: &str) -> bool {
    (1..=32).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// 玩家支持只读汇总；`user` 与 `reference` 必须恰好提供一个。
pub(crate) async fn fetch_player_support(
    user: String,
    reference: String,
) -> Result<PlayerSupportBundle, String> {
    let (key, value) = match (user.trim(), reference.trim()) {
        (user, "") if !user.is_empty() && user.len() <= 256 => ("user", user),
        ("", reference) if valid_uuid(reference) => ("reference", reference),
        _ => return Err("invalid_player_support_query".to_owned()),
    };
    if key == "user" && value.chars().any(char::is_control) {
        return Err("invalid_player_support_query".to_owned());
    }
    let url = format!("/api/v1/admin/player-support?{key}={}", url_encode(value));
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: StrictDeliveryEnvelope<PlayerSupportBundle> =
        response.json().await.map_err(invalid_response_error)?;
    decode_player_support(payload).map_err(|_| invalid_response_error("player_support"))
}

/// 只放行页面实际展示的闭集与有界数值；任何未知种类或越界 ID 都让整页失败关闭。
fn decode_player_support(
    payload: StrictDeliveryEnvelope<PlayerSupportBundle>,
) -> Result<PlayerSupportBundle, ()> {
    if payload.code != "ok" || payload.message.is_empty() {
        return Err(());
    }
    let bundle = payload.data;
    if bundle.bids.len() > MAX_PLAYER_SUPPORT_ROWS
        || bundle.deals.len() > MAX_PLAYER_SUPPORT_ROWS
        || bundle.obligations.len() > MAX_PLAYER_SUPPORT_ROWS
        || bundle.deliveries.len() > MAX_PLAYER_SUPPORT_ROWS
        || bundle.notifications.len() > MAX_PLAYER_SUPPORT_ROWS
    {
        return Err(());
    }
    if let Some(user) = &bundle.user
        && (!valid_uuid(&user.id)
            || user.username.is_empty()
            || !matches!(user.status.as_str(), "active" | "blacklisted")
            || user.game_player_id.as_deref().is_some_and(|value| value.is_empty()))
    {
        return Err(());
    }
    for bid in &bundle.bids {
        if !valid_uuid(&bid.bid_id)
            || !valid_uuid(&bid.lot_id)
            || !valid_uuid(&bid.auction_id)
            || bid.lot_number < 0
            || !valid_world(&bid.world_id)
            || !(0..=MAX_JSON_SAFE_INTEGER).contains(&bid.amount)
            || !matches!(bid.mechanism.as_str(), "ordinary" | "proxy")
            || !matches!(
                bid.lot_status.as_str(),
                "pending" | "live" | "sold" | "unsold" | "withdrawn"
            )
            || parse_delivery_time(&bid.created_at).is_none()
        {
            return Err(());
        }
    }
    for deal in &bundle.deals {
        if !valid_uuid(&deal.lot_id)
            || !valid_uuid(&deal.auction_id)
            || deal.lot_number < 0
            || !valid_world(&deal.world_id)
            || deal.equipment_name.is_empty()
            || !(0..=MAX_JSON_SAFE_INTEGER).contains(&deal.final_price)
        {
            return Err(());
        }
    }
    for obligation in &bundle.obligations {
        if !valid_uuid(&obligation.obligation_id)
            || !valid_uuid(&obligation.auction_id)
            || !valid_uuid(&obligation.lot_id)
            || !valid_world(&obligation.world_id)
            || !valid_safe_text(&obligation.obligation_kind, 64)
            || !matches!(obligation.business_state.as_str(), "ready" | "blocked" | "contingent")
            || !(0..=MAX_JSON_SAFE_INTEGER).contains(&obligation.amount)
            || obligation.delivery_id.as_deref().is_some_and(|id| !valid_uuid(id))
        {
            return Err(());
        }
    }
    for delivery in &bundle.deliveries {
        decode_delivery(delivery.clone())?;
    }
    for notification in &bundle.notifications {
        if !valid_uuid(&notification.notification_id)
            || !matches!(notification.channel.as_str(), "mm" | "pm" | "pending")
            || !valid_safe_text(&notification.state, 64)
            || notification.lot_id.as_deref().is_some_and(|id| !valid_uuid(id))
            || notification.auction_id.as_deref().is_some_and(|id| !valid_uuid(id))
            || notification.delivery_id.as_deref().is_some_and(|id| !valid_uuid(id))
            || notification.state_reason.as_deref().is_some_and(|value| !valid_safe_text(value, 96))
            || parse_delivery_time(&notification.created_at).is_none()
            || parse_delivery_time(&notification.updated_at).is_none()
        {
            return Err(());
        }
    }
    for reference in &bundle.references {
        if !matches!(
            reference.kind.as_str(),
            "delivery" | "obligation" | "notification" | "lot" | "auction"
        ) || !valid_uuid(&reference.id)
        {
            return Err(());
        }
    }
    Ok(bundle)
}

fn valid_world(world: &str) -> bool {
    matches!(world, "main" | "isekai")
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new()).append_pair("v", value).finish()[2..]
        .to_owned()
}

#[cfg(test)]
mod tests;
