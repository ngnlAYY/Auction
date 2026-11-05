//! Admin 投递详情与人工 Sent 证明的同源 HTTP 客户端。
//!
//! 只消费 `auction-contracts` 的安全投影；详情与预览都是只读，确认复用服务端稳定命令 ID
//! 与幂等摘要。浏览器不推导状态机，也不放宽服务端闭集：未知操作类型、越界数字、非规范
//! 摘要或错误投递身份一律 fail-closed。

use auction_contracts::{
    AdminDeliveryDetailResponse, AdminDeliverySentEvidenceRequest,
    AdminDeliverySentProofConfirmRequest, AdminDeliverySentProofPreviewRequest,
    AdminDeliverySentProofPreviewResponse, AdminDeliverySentProofResponse,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::{
    AdminApiFailure, MAX_JSON_SAFE_INTEGER, StrictDeliveryEnvelope, decode_delivery,
    fetch_csrf_token, invalid_request_error, invalid_response_error, parse_delivery_time,
    response_error, response_failure, send_get, send_request, valid_safe_text, valid_success_code,
    valid_uuid,
};

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
            "release" | "retry_failed" | "reconcile" | "sent_operator_confirmed"
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

#[cfg(test)]
mod tests {
    use auction_contracts::{
        AdminDeliveryAttemptResponse, AdminDeliveryCustodyResponse, AdminDeliveryDetailResponse,
        AdminDeliveryEvidenceResponse, AdminDeliveryResponse,
        AdminDeliverySentProofPreviewResponse, AdminDeliverySentProofResponse,
    };
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::{StrictDeliveryEnvelope, decode_confirmation, decode_detail, decode_preview};

    fn time(offset: u32) -> String {
        Utc.with_ymd_and_hms(2026, 11, 2, 1, offset, 0).single().expect("timestamp").to_rfc3339()
    }

    // `is_legacy_uuid` 会校验版本位与变体位，测试夹具必须是规范 UUID。
    const DELIVERY_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const OTHER_ID: &str = "2c9e63a1-7f4b-4a0e-b3d5-9f8c1e2d4b60";

    fn delivery() -> AdminDeliveryResponse {
        AdminDeliveryResponse {
            id: DELIVERY_ID.to_owned(),
            origin_kind: "settlement".to_owned(),
            origin_id: "42".to_owned(),
            recipient: "Restia".to_owned(),
            cod: Some(0),
            status: "manual_reconcile".to_owned(),
            lease_expires_at: None,
            fencing_version: 3,
            attempt_count: 1,
            claimed_at: Some(time(0)),
            sending_at: Some(time(0)),
            completed_at: Some(time(1)),
            external_reference: None,
            failure_code: Some("write_unknown".to_owned()),
            reason_code: None,
            take_required: false,
            returned_mail_id: None,
            created_at: time(0),
            updated_at: time(1),
        }
    }

    fn attempt() -> AdminDeliveryAttemptResponse {
        AdminDeliveryAttemptResponse {
            operation: "send".to_owned(),
            fencing_version: 3,
            world_id: "main".to_owned(),
            sending_at: time(0),
            recipient: Some("Restia".to_owned()),
            subject: Some("Auction settlement".to_owned()),
            body_digest: Some("ab".repeat(32)),
            credits: Some(300_000),
            cod: Some(0),
            equipment_ids: Vec::new(),
            external_mail_id: None,
            credential: None,
            bound_sent_mail_id: None,
        }
    }

    fn detail() -> AdminDeliveryDetailResponse {
        AdminDeliveryDetailResponse {
            delivery: delivery(),
            attempts: vec![attempt()],
            evidence: vec![AdminDeliveryEvidenceResponse {
                reference: "mailbox_sweep:3125345".to_owned(),
                kind: "mailbox_sweep".to_owned(),
                external_mail_id: Some("3125345".to_owned()),
                observed_at: Some(time(2)),
                sent_at: None,
                recipient: Some("Restia".to_owned()),
                subject: Some("Auction settlement".to_owned()),
                body_digest: Some("cd".repeat(32)),
                collected: Some(false),
                cod_pending: Some(0),
                remaining_credits: Some(300_000),
                remaining_equipment_count: Some(0),
                receipt: None,
            }],
            custody: AdminDeliveryCustodyResponse {
                collected: Some(false),
                cod_pending: Some(0),
                remaining_credits: Some(300_000),
                receipt: None,
                observed_at: Some(time(2)),
                operator_confirmed_collected: false,
            },
            dispositions: Vec::new(),
            obligation: None,
            recovery: None,
        }
    }

    #[test]
    fn accepts_complete_detail() {
        let decoded = decode_detail(StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "loaded".to_owned(),
            data: detail(),
        })
        .expect("decode");
        assert_eq!(decoded.attempts.len(), 1);
    }

    #[test]
    fn rejects_send_attempt_without_frozen_fields() {
        let mut data = detail();
        data.attempts[0].subject = None;
        assert!(
            decode_detail(StrictDeliveryEnvelope {
                code: "ok".to_owned(),
                message: "loaded".to_owned(),
                data,
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_evidence_kind_and_uppercase_digest() {
        let mut data = detail();
        data.evidence[0].kind = "draft_discard".to_owned();
        assert!(
            decode_detail(StrictDeliveryEnvelope {
                code: "ok".to_owned(),
                message: "loaded".to_owned(),
                data,
            })
            .is_err()
        );

        let mut data = detail();
        data.evidence[0].body_digest = Some("AB".repeat(32));
        assert!(
            decode_detail(StrictDeliveryEnvelope {
                code: "ok".to_owned(),
                message: "loaded".to_owned(),
                data,
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_take_attempt_with_send_fields_only() {
        let mut data = detail();
        data.attempts[0] = AdminDeliveryAttemptResponse {
            operation: "take".to_owned(),
            external_mail_id: None,
            ..attempt()
        };
        assert!(
            decode_detail(StrictDeliveryEnvelope {
                code: "ok".to_owned(),
                message: "loaded".to_owned(),
                data,
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_preview_for_other_delivery_or_eligible_with_blockers() {
        let preview = |delivery_id: &str, eligible: bool, blockers: Vec<&str>| {
            AdminDeliverySentProofPreviewResponse {
                eligible,
                blockers: blockers.into_iter().map(str::to_owned).collect(),
                delivery_id: delivery_id.to_owned(),
                expected_fencing_version: 3,
                external_reference: "mooglemail:main:send:3125345".to_owned(),
                requires_collected_asset_acceptance: false,
                requires_attempt_history_acceptance: false,
                corrects_admin_failure: true,
            }
        };
        let envelope = |data| StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "preview".to_owned(),
            data,
        };
        assert!(
            decode_preview(envelope(preview(DELIVERY_ID, true, Vec::new())), DELIVERY_ID).is_ok()
        );
        assert!(
            decode_preview(envelope(preview(OTHER_ID, true, Vec::new())), DELIVERY_ID).is_err()
        );
        assert!(
            decode_preview(
                envelope(preview(DELIVERY_ID, true, vec!["sender_player_name_mismatch"])),
                DELIVERY_ID
            )
            .is_err()
        );
    }

    #[test]
    fn confirmation_must_echo_command_and_delivery() {
        let command_id = Uuid::from_bytes([9; 16]);
        let data = AdminDeliverySentProofResponse {
            command_id: command_id.to_string(),
            delivery_id: DELIVERY_ID.to_owned(),
            external_reference: "mooglemail:main:send:3125345".to_owned(),
            replayed: false,
        };
        let envelope = || StrictDeliveryEnvelope {
            code: "ok".to_owned(),
            message: "confirmed".to_owned(),
            data: data.clone(),
        };
        assert!(decode_confirmation(envelope(), command_id, DELIVERY_ID).is_ok());
        assert!(decode_confirmation(envelope(), Uuid::from_bytes([8; 16]), DELIVERY_ID).is_err());
        assert!(decode_confirmation(envelope(), command_id, OTHER_ID).is_err());
    }
}
