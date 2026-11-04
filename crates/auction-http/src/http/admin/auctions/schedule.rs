use auction_application::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use std::net::SocketAddr;

use auction_application::{
    ADMIN_AUCTION_SCHEDULE_COMMAND_KIND, AdminUpdateAuctionScheduleCommand, ApplicationError,
    CommandLedgerError, CompetitionTransactionError, RequestHash,
};
use auction_contracts::{
    AdminUpdateAuctionScheduleRequest, AdminUpdateAuctionScheduleResponse, ApiResponse,
};
use auction_domain::{AuctionId, CommandId};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    AdminApiState, api_error, authenticated_admin_mutation, internal_error, map_auction,
    no_store_headers, parse_canonical_uuid, request_id,
};

/// 修改 Draft/Preview 拍卖的预览、开拍与计划结束时间。
pub(crate) async fn update(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(auction_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "计划修改不接受查询参数")
            .into_response();
    }
    let Some(auction_uuid) = parse_canonical_uuid(&auction_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    };
    let input: AdminUpdateAuctionScheduleRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let Some(command_uuid) = parse_canonical_uuid(&input.command_id) else {
        return invalid_command();
    };
    let (Some(preview_scheduled_at), Some(scheduled_open_at), Some(scheduled_end_at)) =
        (input.preview_scheduled_at, input.scheduled_open_at, input.scheduled_end_at)
    else {
        return invalid_command();
    };
    if input.expected_state_version <= 0 {
        return invalid_command();
    }
    let request_hash = match RequestHash::from_request(
        ADMIN_AUCTION_SCHEDULE_COMMAND_KIND,
        auction_uuid,
        &input,
    ) {
        Ok(hash) => hash,
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_auctions",
                "hash_schedule",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let Some(port) = state.auction_schedule() else {
        return feature_unavailable();
    };
    let result = match port
        .update_schedule(AdminUpdateAuctionScheduleCommand {
            command_id: CommandId::from_uuid(command_uuid),
            auction_id: AuctionId::from_uuid(auction_uuid),
            preview_scheduled_at,
            scheduled_open_at,
            scheduled_end_at,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: request_id(&headers),
            request_hash,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate()
        || result.auction.id.as_uuid() != auction_uuid
        || !matches!(result.auction.status.as_str(), "draft" | "preview")
        || result.auction.preview_scheduled_at != preview_scheduled_at
        || result.auction.scheduled_open_at != scheduled_open_at
        || result.scheduled_ends_at != Some(scheduled_end_at)
    {
        return internal_error();
    }
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok(
            "拍卖计划时间已更新",
            AdminUpdateAuctionScheduleResponse { auction: map_auction(result) },
        )),
    )
        .into_response()
}

fn invalid_command() -> Response {
    api_error(StatusCode::BAD_REQUEST, "invalid_auction_schedule", "拍卖计划修改参数无效")
        .into_response()
}

fn feature_unavailable() -> Response {
    api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 拍卖计划修改仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::Domain(auction_domain::DomainError::InvalidSchedule(_)) => {
            invalid_command()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "not_found", "拍卖不存在").into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::StaleState) => {
            api_error(StatusCode::CONFLICT, "stale_auction_state", "拍卖状态版本已变化")
                .into_response()
        }
        ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::Conflict | CompetitionTransactionError::InvalidState,
        ) => api_error(
            StatusCode::CONFLICT,
            "invalid_auction_state",
            "拍卖状态或计划时间不允许当前修改",
        )
        .into_response(),
        ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin 拍卖计划修改暂不可用",
        )
        .into_response(),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_auctions",
        "respond_schedule_failure",
        diagnostic_kind,
        response,
    )
}
