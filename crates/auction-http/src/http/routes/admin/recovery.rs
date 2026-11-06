//! Admin recovery case 的人工确认处理器。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use crate::http::admin::auctions;
use crate::http::routes::state::AdminApiState;
use crate::http::routes::{
    api_error, authenticated_admin_mutation, internal_error, map_recovery_command_error,
    map_recovery_confirmation, no_store_headers, request_id, strict_json,
};
use auction_application::auction::{AuctionLifecycleActor, AuctionLifecycleActorType};
use auction_application::recovery::{RecoveryConfirmationAction, RecoveryConfirmationCommand};
use auction_contracts::{
    AdminRecoveryConfirmationRequest, AdminRecoveryConfirmationResponse, ApiResponse,
};
use auction_domain::{CommandId, SettlementId};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;

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

    let Some(recovery_uuid) = auctions::parse_canonical_uuid(&recovery_id) else {
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
    let Some(command_uuid) = auctions::parse_canonical_uuid(&input.command_id) else {
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
        return api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "恢复确认仅支持 PostgreSQL 完整档",
        )
        .into_response();
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
