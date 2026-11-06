//! Admin 人工 Sent 证明：选择已写入投递的维护证据，复用 CLI 同一命令与幂等摘要。
//!
//! 预览无副作用；确认只追加 Sent 证明事实，不会重发邮件、创建新资金来源或改写原失败
//! 命令。证据引用只用于准入核对，不进入命令摘要，因此与 CLI 的 `command_id` 语义一致。

use std::net::SocketAddr;

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAdminError, ManualSentEvidence, SentConfirmationCommand,
};
use auction_contracts::{
    AdminDeliverySentEvidenceRequest, AdminDeliverySentProofConfirmRequest, ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{
    AdminApiState, api_error, authenticated_admin_mutation, feature_unavailable,
    map_sent_proof_preview, map_sent_proof_result, no_store_headers,
};

/// 只读预览：返回闭集 blockers，不写入任何事实。
pub(crate) async fn preview(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Err(response) = authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "交付命令不接受查询参数")
            .into_response();
    }
    let Ok(delivery_id) = parse_delivery_id(&delivery_id) else {
        return invalid_delivery_id();
    };
    let Ok(input) =
        serde_json::from_slice::<auction_contracts::AdminDeliverySentProofPreviewRequest>(&body)
    else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
            .into_response();
    };
    let Ok(evidence) = manual_evidence(input.evidence) else {
        return invalid_evidence();
    };
    let Some(service) = state.delivery_sent_proof() else {
        return feature_unavailable();
    };
    let preview = match service.preview(delivery_id, evidence, &input.evidence_reference).await {
        Ok(preview) => preview,
        Err(error) => return map_sent_proof_error(error),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("人工 Sent 证明预览", map_sent_proof_preview(preview))),
    )
        .into_response()
}

/// 确认：`command_id` 与同参 CLI 命令共享摘要，重放返回原结果。
pub(crate) async fn confirm(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(delivery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "交付命令不接受查询参数")
            .into_response();
    }
    let Ok(delivery_id) = parse_delivery_id(&delivery_id) else {
        return invalid_delivery_id();
    };
    let Ok(input) = serde_json::from_slice::<AdminDeliverySentProofConfirmRequest>(&body) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
            .into_response();
    };
    if input.command_id.is_nil() || input.expected_fencing_version <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "交付命令参数无效")
            .into_response();
    }
    let Ok(evidence) = manual_evidence(input.evidence) else {
        return invalid_evidence();
    };
    let command = SentConfirmationCommand {
        command_id: input.command_id,
        delivery_id,
        expected_fencing_version: input.expected_fencing_version,
        actor: identity.actor.clone(),
        // 主机身份属于 CLI 运维元数据；Admin 请求不携带，也不影响命令摘要。
        operator_identity: None,
        reason: input.reason,
        evidence,
        accept_collected_assets: input.accept_collected_assets,
        accept_attempt_history: input.accept_attempt_history,
    };
    if command.validate().is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "交付命令参数无效")
            .into_response();
    }
    let Some(service) = state.delivery_sent_proof() else {
        return feature_unavailable();
    };
    let result = match service.confirm(command, &input.evidence_reference).await {
        Ok(result) => result,
        Err(error) => return map_sent_proof_error(error),
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("人工 Sent 证明已提交", map_sent_proof_result(result))),
    )
        .into_response()
}

fn parse_delivery_id(value: &str) -> Result<Uuid, ()> {
    let delivery_id = Uuid::parse_str(value).map_err(|error| {
        let _ = error;
    })?;
    if delivery_id.is_nil() {
        return Err(());
    }
    Ok(delivery_id)
}

fn invalid_delivery_id() -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_delivery_id", "交付 ID 无效").into_response()
}

/// 合同的 32 字节摘要与 CLI 使用同一数组编码；时间必须是带时区的 RFC3339。
fn manual_evidence(input: AdminDeliverySentEvidenceRequest) -> Result<ManualSentEvidence, ()> {
    let time = |value: &str| {
        DateTime::parse_from_rfc3339(value).map(|time| time.with_timezone(&Utc)).map_err(|error| {
            let _ = error;
        })
    };
    Ok(ManualSentEvidence {
        schema_version: input.schema_version,
        world_id: input.world_id,
        external_mail_id: input.external_mail_id,
        sender_uid: input.sender_uid,
        sender_name: input.sender_name,
        recipient: input.recipient,
        subject: input.subject,
        sent_minute: time(&input.sent_minute)?,
        observed_at: time(&input.observed_at)?,
        original_body_digest: input.original_body_digest,
        observed_body_digest: input.observed_body_digest,
        remaining_credits: input.remaining_credits,
        cod_pending: input.cod_pending,
        equipment_ids: input.equipment_ids,
        collected: input.collected,
        receipt: input.receipt,
    })
}

fn invalid_evidence() -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_delivery_command", "交付命令参数无效")
        .into_response()
}

fn map_sent_proof_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::Invalid) => invalid_evidence(),
        ApplicationError::DeliveryAdmin(DeliveryAdminError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "not_found", "交付不存在").into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::Stale) => {
            api_error(StatusCode::CONFLICT, "stale_delivery_state", "交付状态或版本已变化")
                .into_response()
        }
        ApplicationError::DeliveryAdmin(DeliveryAdminError::CommandConflict) => {
            api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
                .into_response()
        }
        ApplicationError::FulfillmentDependencyMismatch => api_error(
            StatusCode::CONFLICT,
            "sent_proof_rejected",
            "人工 Sent 证明与冻结身份或当前状态不匹配",
        )
        .into_response(),
        _ => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
                .into_response()
        }
    }
}
