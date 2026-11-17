//! Admin recovery case 的人工确认处理器。

use std::net::SocketAddr;

use auction_application::auction::{AuctionLifecycleActor, AuctionLifecycleActorType};
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::recovery::{
    RecoveryConfirmationAction, RecoveryConfirmationCommand, RecoveryConfirmationResult,
};
use auction_contracts::{
    AdminRecoveryConfirmationRequest, AdminRecoveryConfirmationResponse,
    AdminRecoveryConfirmationResultResponse, ApiResponse,
};
use auction_domain::{CommandId, SettlementId};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

use crate::diagnostics;
use crate::errors::{DATABASE_NOT_CONFIGURED, api_error, feature_unavailable};
use crate::middleware::boundary::request_id;
use crate::middleware::strict_json;
use crate::query::parse_canonical_uuid;
use crate::session::{
    authenticated_admin_mutation, internal_error, map_recovery_command_error, no_store_headers,
};
use crate::state::AdminApiState;

/// 以版本 fence 确认一个已解决 custody 证据的 recovery case。
pub(crate) async fn admin_confirm_recovery(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(recovery_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };

    let Some(recovery_uuid) = parse_canonical_uuid(&recovery_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
            .into_response();
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "恢复确认不接受查询参数")
            .into_response();
    }
    if !strict_json::application_json(&headers) {
        return api_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "请求必须使用 JSON",
        )
        .into_response();
    }
    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    // Go encoding/json 会把整个 struct 请求的 null 解码为零值，再由业务边界返回命令无效。
    let input: AdminRecoveryConfirmationRequest = if value.is_null() {
        AdminRecoveryConfirmationRequest::default()
    } else {
        match serde_json::from_value(value) {
            Ok(input) => input,
            Err(_) => {
                return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                    .into_response();
            }
        }
    };
    let Some(command_uuid) = parse_canonical_uuid(&input.command_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
            .into_response();
    };
    if input.expected_state_version <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
            .into_response();
    }
    let Some(action) = RecoveryConfirmationAction::parse(&input.action) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_recovery_command", "恢复确认参数无效")
            .into_response();
    };
    let request_hash = match RecoveryConfirmationCommand::legacy_request_hash(
        recovery_uuid,
        input.expected_state_version,
        action,
        &input.reason,
    ) {
        Ok(hash) => hash,
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "admin_recovery",
                "hash_confirmation",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let Some(commands) = state.recovery_commands() else {
        return feature_unavailable(DATABASE_NOT_CONFIGURED);
    };
    let command = RecoveryConfirmationCommand {
        command_id: CommandId::from_uuid(command_uuid),
        recovery_id: SettlementId::from_uuid(recovery_uuid),
        expected_state_version: input.expected_state_version,
        action,
        reason: input.reason,
        request_hash,
        request_id: request_id(&headers),
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: identity.actor,
        },
    };
    let result = match commands.confirm_recovery(command).await {
        Ok(result) => result,
        Err(error) => return map_recovery_command_error(error),
    };
    if result.recovery_id.as_uuid() != recovery_uuid
        || result.state_version <= input.expected_state_version
    {
        return internal_error();
    }
    let data = AdminRecoveryConfirmationResponse { recovery: map_recovery_confirmation(result) };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("恢复确认已提交", data)))
        .into_response()
}

fn map_recovery_confirmation(
    result: RecoveryConfirmationResult,
) -> AdminRecoveryConfirmationResultResponse {
    AdminRecoveryConfirmationResultResponse {
        recovery_id: result.recovery_id.to_string(),
        action: result.action.as_str().to_owned(),
        stage: result.stage,
        resolution: result.resolution,
        state_version: result.state_version,
        relisted_auction_id: result.relisted_auction_id.map(|id| id.to_string()),
        relisted_lot_id: result.relisted_lot_id.map(|id| id.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use auction_application::core::ApplicationError;
    use auction_application::recovery::RecoveryCommandError;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    use crate::session::map_recovery_command_error;

    async fn mapped(error: RecoveryCommandError) -> (StatusCode, serde_json::Value) {
        let response = map_recovery_command_error(ApplicationError::RecoveryCommand(error));
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("读取错误响应体");
        (status, serde_json::from_slice(&body).expect("错误响应是 JSON"))
    }

    /// 退件不能唯一归属到一次发送是明确的业务阻断：精确 409 与独立错误码，交人工核对；既有
    /// 恢复确认的冲突映射保持不变。
    #[tokio::test]
    async fn send_attribution_block_maps_to_an_explicit_conflict() {
        for (error, code) in [
            (RecoveryCommandError::SendUnattributed, "recovery_send_unattributed"),
            (RecoveryCommandError::NotConfirmable, "recovery_not_confirmable"),
            (RecoveryCommandError::EvidenceUnresolved, "recovery_evidence_unresolved"),
            (RecoveryCommandError::StaleState, "stale_recovery_state"),
        ] {
            let label = format!("{error:?}");
            let (status, body) = mapped(error).await;
            assert_eq!(
                (status, body["code"].as_str()),
                (StatusCode::CONFLICT, Some(code)),
                "{label}"
            );
        }
    }
}
