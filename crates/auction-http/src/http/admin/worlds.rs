//! Admin 业务世界目录与启停 HTTP 适配器。

use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::admin::{AdminWorldError, AdminWorldSetEnabledCommand};
use auction_application::auction::WorldSnapshot;
use auction_application::core::{
    ApplicationError, CommandLedgerError, CompetitionTransactionError,
};
use auction_contracts::{
    AdminWorldListResponse, AdminWorldResponse, AdminWorldSetEnabledRequest,
    AdminWorldSetEnabledResponse, ApiResponse,
};
use auction_domain::CommandId;
use axum::{
    Json,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, api_error, authenticated_admin_mutation, authenticated_admin_session,
    internal_error, no_store_headers, parse_query, request_id,
};

pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if parse_query(&uri, &[]).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "业务世界目录不接受查询参数")
            .into_response();
    }
    let Some(port) = state.admin_worlds() else {
        return unavailable();
    };
    let worlds = match port.list_worlds().await {
        Ok(worlds) => worlds,
        Err(error) => return map_error(error),
    };
    let Some(worlds) = worlds.into_iter().map(map_world).collect::<Option<Vec<_>>>() else {
        return internal_error();
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("业务世界状态已加载", AdminWorldListResponse { worlds })),
    )
        .into_response()
}

pub(crate) async fn set_enabled(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(world_id): Path<String>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let uri = parts.uri;
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if parse_query(&uri, &[]).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "业务世界命令不接受查询参数")
            .into_response();
    }
    if !matches!(world_id.as_str(), "main" | "isekai") {
        return api_error(StatusCode::BAD_REQUEST, "invalid_world", "业务世界无效").into_response();
    }
    let input: AdminWorldSetEnabledRequest =
        match super::strict_json::decode_after_guard(&headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    if input.command_id.is_nil() || input.expected_policy_revision <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_world_command", "业务世界命令无效")
            .into_response();
    }
    let Some(port) = state.admin_worlds() else {
        return unavailable();
    };
    let result = match port
        .set_enabled(AdminWorldSetEnabledCommand {
            command_id: CommandId::from_uuid(input.command_id),
            world_id,
            enabled: input.enabled,
            expected_policy_revision: input.expected_policy_revision,
            actor_id: identity.actor,
            request_id: request_id(&headers),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    let Some(world) = map_world(result) else {
        return internal_error();
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("业务世界状态已更新", AdminWorldSetEnabledResponse { world })),
    )
        .into_response()
}

fn map_world(world: WorldSnapshot) -> Option<AdminWorldResponse> {
    world.validate().then_some(AdminWorldResponse {
        id: world.id,
        enabled: world.enabled,
        availability: world.availability,
        policy_revision: world.policy_revision,
    })
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminWorld(AdminWorldError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_world_command", "业务世界命令无效")
                .into_response()
        }
        ApplicationError::AdminWorld(AdminWorldError::NotFound)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "world_not_found", "业务世界不存在").into_response()
        }
        ApplicationError::AdminWorld(AdminWorldError::ActiveAuction) => api_error(
            StatusCode::CONFLICT,
            "world_has_active_auction",
            "该世界仍有未结束的拍卖，暂时不能关闭",
        )
        .into_response(),
        ApplicationError::AdminWorld(AdminWorldError::PolicyRevisionConflict { .. })
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict) => {
            api_error(StatusCode::CONFLICT, "stale_world_state", "业务世界状态已经变化")
                .into_response()
        }
        ApplicationError::AdminWorld(AdminWorldError::CommandConflict)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CapabilityUnavailable(_) => api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "当前部署档位不支持业务世界启停",
        )
        .into_response(),
        _ => unavailable(),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_worlds",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}

fn unavailable() -> Response {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "业务世界状态暂不可用")
        .into_response()
}
