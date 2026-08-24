//! Admin 拍卖 HTTP 适配器。
//!
//! 这里保留旧分页和筛选契约，只负责认证、参数边界和安全 DTO 映射；拍卖生命周期及 lot
//! 统计由应用端口在同一 PostgreSQL 快照中读取，创建命令由应用端口负责幂等和事务编排。

use std::net::SocketAddr;

use auction_application::{
    AdminAuctionListRequest, AdminAuctionPage, AdminAuctionSummary, AdminCreateAuctionCommand,
    ApplicationError, CommandLedgerError, CompetitionTransactionError, MAX_ADMIN_AUCTION_OFFSET,
    MAX_ADMIN_AUCTION_PAGE_SIZE, RequestHash,
};
use auction_contracts::{
    AdminAuctionCapabilitiesResponse, AdminAuctionCreateResponse, AdminAuctionListResponse,
    AdminAuctionPaginationResponse, AdminAuctionResponse, AdminCreateAuctionRequest, ApiResponse,
};
use auction_domain::{AuctionId, CommandId};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 创建 Admin draft 拍卖。
pub(crate) async fn create(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    let Some(raw_csrf) = super::read_admin_cookie(&headers, super::ADMIN_CSRF_COOKIE) else {
        return super::authentication_error();
    };
    if !super::valid_admin_csrf_header(&headers, &raw_csrf) {
        return super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
            .into_response();
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "创建拍卖不接受查询参数",
        )
        .into_response();
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let input: AdminCreateAuctionRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    if input.auction_id.is_nil() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            .into_response();
    }
    if input.expected_state_version != 0 {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_state_version",
            "创建拍卖必须使用零版本 fence",
        )
        .into_response();
    }
    let request_hash =
        match RequestHash::from_request("admin_auction_create", input.auction_id, &input) {
            Ok(hash) => hash,
            Err(_) => return super::internal_error(),
        };
    let Some(port) = state.auction_create() else {
        return create_feature_unavailable();
    };
    let result = match port
        .create(AdminCreateAuctionCommand {
            command_id: CommandId::from_uuid(input.command_id),
            auction_id: AuctionId::from_uuid(input.auction_id),
            world_id: input.world_id.clone(),
            sequence: input.sequence,
            preview_scheduled_at: input.preview_scheduled_at,
            scheduled_open_at: input.scheduled_open_at,
            scheduled_end_at: input.scheduled_end_at,
            expected_state_version: input.expected_state_version,
            actor_id: identity.actor,
            request_id: super::request_id(&headers),
            request_hash,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_create_error(error),
    };
    if !result.validate()
        || result.auction.id.as_uuid() != input.auction_id
        || result.auction.world_id != input.world_id
        || result.auction.sequence != input.sequence
        || result.auction.status != "draft"
        || result.auction.state_version != 1
    {
        return super::internal_error();
    }
    (
        StatusCode::CREATED,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "后台拍卖已创建",
            AdminAuctionCreateResponse { auction: map_auction(result) },
        )),
    )
        .into_response()
}

/// 读取 Admin 拍卖列表。
pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = super::authenticated_admin_session(&state, &headers).await {
        return *response;
    }

    let query = match super::parse_query(&uri, &["world_id", "status", "page", "page_size"]) {
        Ok(query) => query,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page = match super::parse_positive_i64(query.get("page"), 1, None) {
        Ok(page) => page,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let page_size = match super::parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PAGE_SIZE,
        Some(MAX_ADMIN_AUCTION_PAGE_SIZE),
    ) {
        Ok(page_size) => page_size,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let offset = match page.checked_sub(1).and_then(|value| value.checked_mul(page_size)) {
        Some(offset) if offset <= MAX_ADMIN_AUCTION_OFFSET => offset,
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
    };
    let request = AdminAuctionListRequest {
        world_id: query.get("world_id").cloned(),
        status: query.get("status").cloned(),
        limit: page_size,
        offset,
    };
    if !request.validate() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
            .into_response();
    }
    let Some(port) = state.admin_auctions() else {
        return feature_unavailable();
    };
    let result = match port.list_admin_auctions(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !valid_page(&result, &request) {
        return super::internal_error();
    }

    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "拍卖列表已加载",
            AdminAuctionListResponse {
                auctions: result.entries.into_iter().map(map_auction).collect(),
                capabilities: AdminAuctionCapabilitiesResponse {
                    cancel_auction: true,
                    manual_early_open: true,
                    manual_intake: state.admin_intake().is_some(),
                    manual_preview_open: true,
                },
                pagination: AdminAuctionPaginationResponse { page, page_size, total: result.total },
            },
        )),
    )
        .into_response()
}

fn valid_page(page: &AdminAuctionPage, request: &AdminAuctionListRequest) -> bool {
    page.validate(request)
}

fn map_auction(summary: AdminAuctionSummary) -> AdminAuctionResponse {
    let auction = summary.auction;
    AdminAuctionResponse {
        id: auction.id.to_string(),
        world_id: auction.world_id,
        sequence: auction.sequence,
        status: auction.status,
        preview_scheduled_at: super::timestamp(auction.preview_scheduled_at),
        scheduled_open_at: super::timestamp(auction.scheduled_open_at),
        preview_opened_at: auction.preview_opened_at.map(super::timestamp),
        preview_manual_open: auction.preview_manual_open,
        preview_revision: auction.preview_revision,
        opened_at: auction.opened_at.map(super::timestamp),
        final_phase_at: auction.final_phase_at.map(super::timestamp),
        settled_at: auction.settled_at.map(super::timestamp),
        cancelled_at: auction.cancelled_at.map(super::timestamp),
        cancelled_reason: auction.cancelled_reason,
        cancelled_by: auction.cancelled_by,
        state_version: auction.state_version,
        scheduled_end_at: auction.scheduled_end_at.map(super::timestamp),
        effective_end_at: auction.effective_end_at.map(super::timestamp),
        scheduled_ends_at: summary.scheduled_ends_at.map(super::timestamp),
        lot_count: summary.lot_count,
        created_at: super::timestamp(auction.created_at),
        updated_at: super::timestamp(auction.updated_at),
    }
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 拍卖列表仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn create_feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 创建拍卖仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn map_create_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::Domain(auction_domain::DomainError::InvalidSchedule(_)) => {
            super::api_error(
                StatusCode::BAD_REQUEST,
                "invalid_auction_create",
                "后台拍卖创建参数无效",
            )
            .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict) => {
            super::api_error(StatusCode::CONFLICT, "auction_conflict", "拍卖创建与现有状态冲突")
                .into_response()
        }
        ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CapabilityUnavailable(_) => create_feature_unavailable(),
        ApplicationError::Serialization(_) => super::internal_error(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin 创建拍卖暂不可用",
        )
        .into_response(),
    }
}

fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin 拍卖列表暂不可用",
        )
        .into_response(),
    }
}
