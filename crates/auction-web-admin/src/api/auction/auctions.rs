//! Admin 拍卖列表 API 客户端。
//!
//! 页面只接收后端的生命周期安全投影；创建命令携带新的 `command_id`，状态与幂等仍由服务端负责。

pub(crate) use auction_contracts::{
    AdminAuctionCapabilitiesResponse as AdminAuctionCapabilities,
    AdminAuctionListResponse as AdminAuctionPage, AdminAuctionResponse as AdminAuction,
};
use auction_contracts::{
    AdminAuctionCommandResponse, AdminAuctionPaginationResponse, AdminCreateAuctionRequest,
};
use chrono::{DateTime, Utc};
use gloo_net::http::{Request, Response};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, OptionalField, RequiredOption, TABLE_PAGE_SIZE,
    invalid_request_error, invalid_response_error, parse_admin_time, response_error, send_get,
    send_request, valid_admin_time, valid_trimmed_text, valid_uuid,
};

/// 旧 Admin 列表和生命周期命令共用同一拍卖 decoder；两个列表扩展字段允许缺失。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StrictAuction {
    cancelled_at: RequiredOption<String>,
    cancelled_by: RequiredOption<String>,
    cancelled_reason: RequiredOption<String>,
    created_at: String,
    final_phase_at: RequiredOption<String>,
    id: String,
    #[serde(default)]
    lot_count: OptionalField<i64>,
    opened_at: RequiredOption<String>,
    preview_manual_open: bool,
    preview_opened_at: RequiredOption<String>,
    preview_revision: i64,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_ends_at: Option<String>,
    sequence: i64,
    settled_at: RequiredOption<String>,
    state_version: i64,
    status: String,
    updated_at: String,
    world_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAuctionCapabilities {
    cancel_auction: bool,
    manual_early_open: bool,
    manual_early_settlement: bool,
    manual_intake: bool,
    manual_preview_open: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictPagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAuctionList {
    auctions: Vec<StrictAuction>,
    capabilities: StrictAuctionCapabilities,
    pagination: StrictPagination,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictAuctionData {
    auction: StrictAuction,
}

pub(crate) async fn fetch_admin_auctions(
    world_id: String,
    status: String,
    page: i64,
) -> Result<AdminAuctionPage, String> {
    fetch_admin_auctions_with_size(world_id, status, page, TABLE_PAGE_SIZE).await
}

async fn fetch_admin_auctions_with_size(
    world_id: String,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AdminAuctionPage, String> {
    let mut url = format!("/api/v1/admin/auctions?page={page}&page_size={page_size}");
    if !world_id.is_empty() && world_id != "all" {
        url.push_str("&world_id=");
        url.push_str(&world_id);
    }
    if status != "all" {
        url.push_str("&status=");
        url.push_str(&status);
    }
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictAuctionList> =
        response.json().await.map_err(invalid_response_error)?;
    decode_auction_list(payload.data).map_err(invalid_response_error)
}

/// 提交 Admin draft 创建命令；调用组件负责在同一输入重试时复用两个稳定标识。
pub(crate) async fn create_admin_auction(
    command_id: Uuid,
    auction_id: Uuid,
    world_id: String,
    sequence: i64,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_end_at: String,
) -> Result<AdminAuctionCommandResponse, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let command = AdminCreateAuctionRequest {
        command_id: command_id.to_string(),
        auction_id: auction_id.to_string(),
        world_id,
        sequence,
        preview_scheduled_at: Some(normalize_datetime(preview_scheduled_at)?),
        scheduled_open_at: Some(normalize_datetime(scheduled_open_at)?),
        scheduled_end_at: Some(normalize_datetime(scheduled_end_at)?),
        expected_state_version: 0,
    };
    let response = send_request(|abort_signal| {
        Request::post("/api/v1/admin/auctions")
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let auction = decode_auction_command_response(response).await?;
    if auction.id != auction_id.to_string() || auction.status != "draft" {
        return Err("invalid_response".to_owned());
    }
    Ok(auction)
}

/// 生命周期与创建命令共用旧版 `{ auction }` 闭集 decoder。
pub(super) async fn decode_auction_command_response(
    response: Response,
) -> Result<AdminAuctionCommandResponse, String> {
    let payload: ApiResponse<StrictAuctionData> =
        response.json().await.map_err(invalid_response_error)?;
    decode_auction(payload.data.auction).map(auction_into_command).map_err(invalid_response_error)
}

fn decode_auction_list(data: StrictAuctionList) -> Result<AdminAuctionPage, &'static str> {
    let pagination = decode_pagination(data.pagination)?;
    let auctions = data.auctions.into_iter().map(decode_auction).collect::<Result<Vec<_>, _>>()?;
    Ok(AdminAuctionPage {
        auctions,
        capabilities: AdminAuctionCapabilities {
            cancel_auction: data.capabilities.cancel_auction,
            manual_early_open: data.capabilities.manual_early_open,
            manual_early_settlement: data.capabilities.manual_early_settlement,
            manual_intake: data.capabilities.manual_intake,
            manual_preview_open: data.capabilities.manual_preview_open,
        },
        pagination,
    })
}

pub(super) fn decode_auction(value: StrictAuction) -> Result<AdminAuction, &'static str> {
    let preview_scheduled_at =
        parse_admin_time(&value.preview_scheduled_at).ok_or("auction_preview_time")?;
    let scheduled_open_at =
        parse_admin_time(&value.scheduled_open_at).ok_or("auction_open_time")?;
    let cancellation_fields_present = value.cancelled_at.0.is_some()
        && value.cancelled_by.0.is_some()
        && value.cancelled_reason.0.is_some();
    let cancellation_fields_absent = value.cancelled_at.0.is_none()
        && value.cancelled_by.0.is_none()
        && value.cancelled_reason.0.is_none();
    if !valid_uuid(&value.id)
        || !valid_trimmed_text(&value.world_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.sequence)
        || !matches!(
            value.status.as_str(),
            "draft" | "preview" | "live" | "final" | "settling" | "settled" | "cancelled"
        )
        || scheduled_open_at < preview_scheduled_at
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.preview_revision)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.state_version)
        || !valid_admin_time(&value.created_at)
        || !valid_admin_time(&value.updated_at)
        || !valid_optional_time(value.cancelled_at.0.as_deref())
        || !valid_optional_text(value.cancelled_by.0.as_deref())
        || !valid_optional_text(value.cancelled_reason.0.as_deref())
        || !valid_optional_time(value.preview_opened_at.0.as_deref())
        || !valid_optional_time(value.opened_at.0.as_deref())
        || !valid_optional_time(value.final_phase_at.0.as_deref())
        || !valid_optional_time(value.settled_at.0.as_deref())
        || !valid_optional_time(value.scheduled_ends_at.as_deref())
        || value.lot_count.0.is_some_and(|count| !(0..=MAX_JSON_SAFE_INTEGER).contains(&count))
        || if value.status == "cancelled" {
            !cancellation_fields_present
        } else {
            !cancellation_fields_absent
        }
    {
        return Err("auction");
    }

    Ok(AdminAuction {
        id: value.id,
        world_id: value.world_id,
        sequence: value.sequence,
        status: value.status,
        preview_scheduled_at: value.preview_scheduled_at,
        scheduled_open_at: value.scheduled_open_at,
        preview_opened_at: value.preview_opened_at.0,
        preview_manual_open: value.preview_manual_open,
        preview_revision: value.preview_revision,
        opened_at: value.opened_at.0,
        final_phase_at: value.final_phase_at.0,
        settled_at: value.settled_at.0,
        cancelled_at: value.cancelled_at.0,
        cancelled_reason: value.cancelled_reason.0,
        cancelled_by: value.cancelled_by.0,
        state_version: value.state_version,
        scheduled_ends_at: value.scheduled_ends_at,
        lot_count: value.lot_count.0,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })
}

fn decode_pagination(
    value: StrictPagination,
) -> Result<AdminAuctionPaginationResponse, &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.total)
    {
        return Err("pagination");
    }
    Ok(AdminAuctionPaginationResponse {
        page: value.page,
        page_size: value.page_size,
        total: value.total,
    })
}

fn auction_into_command(value: AdminAuction) -> AdminAuctionCommandResponse {
    AdminAuctionCommandResponse {
        id: value.id,
        world_id: value.world_id,
        sequence: value.sequence,
        status: value.status,
        preview_scheduled_at: value.preview_scheduled_at,
        scheduled_open_at: value.scheduled_open_at,
        preview_opened_at: value.preview_opened_at,
        preview_manual_open: value.preview_manual_open,
        preview_revision: value.preview_revision,
        opened_at: value.opened_at,
        final_phase_at: value.final_phase_at,
        settled_at: value.settled_at,
        cancelled_at: value.cancelled_at,
        cancelled_reason: value.cancelled_reason,
        cancelled_by: value.cancelled_by,
        state_version: value.state_version,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

fn valid_optional_time(value: Option<&str>) -> bool {
    value.is_none_or(valid_admin_time)
}

fn valid_optional_text(value: Option<&str>) -> bool {
    value.is_none_or(valid_trimmed_text)
}

fn normalize_datetime(value: String) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(&super::normalize_datetime(&value))
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| "invalid_request".to_owned())
}
