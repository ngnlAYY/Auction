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

use self::decode::StrictDeliveryEnvelope;
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

mod player_support;
mod responses;
#[cfg(test)]
mod tests;
use player_support::*;
use responses::*;
