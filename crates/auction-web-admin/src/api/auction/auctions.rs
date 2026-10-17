//! Admin 拍卖列表 API 客户端。
//!
//! 页面只接收后端的生命周期安全投影；创建命令携带新的 `command_id`，状态与幂等仍由服务端负责。

pub(crate) use auction_contracts::{
    AdminAuctionCapabilitiesResponse as AdminAuctionCapabilities,
    AdminAuctionListResponse as AdminAuctionPage, AdminAuctionResponse as AdminAuction,
};
use auction_contracts::{
    AdminAuctionCommandResponse, AdminAuctionLifecycleResponse, AdminAuctionPaginationResponse,
    AdminCreateAuctionRequest, AdminUpdateAuctionScheduleRequest,
    AdminUpdateAuctionScheduleResponse,
};
use chrono::{DateTime, Utc};
use gloo_net::http::{Request, Response};
use uuid::Uuid;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, invalid_request_error, invalid_response_error,
    parse_admin_time, response_error, send_get, send_request, valid_admin_time, valid_trimmed_text,
    valid_uuid,
};

// 旧 Admin 列表和生命周期命令共用同一拍卖 decoder；contracts 侧的 `AdminAuctionResponse`
// 与 `AdminAuctionCommandResponse` 已带 `deny_unknown_fields` 与必填 nullable 语义，本文件
// 不再保留本地 Strict 镜像，只保留跨字段业务谓词。

pub(crate) async fn fetch_admin_auctions(
    world_id: String,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AdminAuctionPage, String> {
    fetch_admin_auctions_with_size(world_id, status, page, page_size).await
}

async fn fetch_admin_auctions_with_size(
    world_id: String,
    status: String,
    page: i64,
    page_size: i64,
) -> Result<AdminAuctionPage, String> {
    super::pagination::validate(page, page_size, 100, 100_000)?;
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
    let payload: ApiResponse<AdminAuctionPage> =
        response.json().await.map_err(invalid_response_error)?;
    let result = decode_auction_list(payload.data).map_err(invalid_response_error)?;
    if result.pagination.page != page
        || result.pagination.page_size != page_size
        || i64::try_from(result.auctions.len())
            .map_or(true, |count| count > page_size || count > result.pagination.total)
    {
        return Err(invalid_response_error("auction_pagination"));
    }
    Ok(result)
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

/// 修改 Draft/Preview 拍卖的三项计划时间；同一输入重试时由组件复用命令 ID。
pub(crate) async fn update_admin_auction_schedule(
    command_id: Uuid,
    auction_id: String,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_end_at: String,
    expected_state_version: i64,
) -> Result<AdminAuction, String> {
    if !valid_uuid(&auction_id) || expected_state_version <= 0 {
        return Err("invalid_request".to_owned());
    }
    let normalized_preview = normalize_datetime(preview_scheduled_at)?;
    let normalized_open = normalize_datetime(scheduled_open_at)?;
    let normalized_end = normalize_datetime(scheduled_end_at)?;
    let expected_preview = normalized_preview.timestamp_millis();
    let expected_open = normalized_open.timestamp_millis();
    let expected_end = normalized_end.timestamp_millis();
    let command = AdminUpdateAuctionScheduleRequest {
        command_id: command_id.to_string(),
        preview_scheduled_at: Some(normalized_preview),
        scheduled_open_at: Some(normalized_open),
        scheduled_end_at: Some(normalized_end),
        expected_state_version,
    };
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/auctions/{auction_id}/schedule");
    let response = send_request(|abort_signal| {
        Request::patch(&url)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .json(&command)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminUpdateAuctionScheduleResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let auction = decode_auction(payload.data.auction).map_err(invalid_response_error)?;
    if auction.id != auction_id
        || !matches!(auction.status.as_str(), "draft" | "preview")
        || auction.state_version < expected_state_version
        || parse_admin_time(&auction.preview_scheduled_at).map(|value| value as i64)
            != Some(expected_preview)
        || parse_admin_time(&auction.scheduled_open_at).map(|value| value as i64)
            != Some(expected_open)
        || auction.scheduled_ends_at.as_deref().and_then(parse_admin_time).map(|value| value as i64)
            != Some(expected_end)
    {
        return Err("invalid_response".to_owned());
    }
    Ok(auction)
}

/// 生命周期与创建命令共用旧版 `{ auction }` 闭集 decoder。
pub(super) async fn decode_auction_command_response(
    response: Response,
) -> Result<AdminAuctionCommandResponse, String> {
    let payload: ApiResponse<AdminAuctionLifecycleResponse> =
        response.json().await.map_err(invalid_response_error)?;
    decode_auction(command_into_auction(payload.data.auction))
        .map(auction_into_command)
        .map_err(invalid_response_error)
}

fn decode_auction_list(data: AdminAuctionPage) -> Result<AdminAuctionPage, &'static str> {
    decode_pagination(&data.pagination)?;
    let auctions = data.auctions.into_iter().map(decode_auction).collect::<Result<Vec<_>, _>>()?;
    Ok(AdminAuctionPage { auctions, capabilities: data.capabilities, pagination: data.pagination })
}

/// 旧版命令投影缺少列表扩展字段；补为缺失值后复用同一闭集谓词。
fn command_into_auction(value: AdminAuctionCommandResponse) -> AdminAuction {
    AdminAuction {
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
        scheduled_ends_at: None,
        lot_count: None,
        created_at: value.created_at,
        updated_at: value.updated_at,
    }
}

/// 跨字段谓词保留为独立校验：时间顺序、cancelled 字段组合与值域仍在浏览器边界 fail-closed。
pub(super) fn decode_auction(value: AdminAuction) -> Result<AdminAuction, &'static str> {
    let preview_scheduled_at =
        parse_admin_time(&value.preview_scheduled_at).ok_or("auction_preview_time")?;
    let scheduled_open_at =
        parse_admin_time(&value.scheduled_open_at).ok_or("auction_open_time")?;
    let cancellation_fields_present = value.cancelled_at.is_some()
        && value.cancelled_by.is_some()
        && value.cancelled_reason.is_some();
    let cancellation_fields_absent = value.cancelled_at.is_none()
        && value.cancelled_by.is_none()
        && value.cancelled_reason.is_none();
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
        || !valid_optional_time(value.cancelled_at.as_deref())
        || !valid_optional_text(value.cancelled_by.as_deref())
        || !valid_optional_text(value.cancelled_reason.as_deref())
        || !valid_optional_time(value.preview_opened_at.as_deref())
        || !valid_optional_time(value.opened_at.as_deref())
        || !valid_optional_time(value.final_phase_at.as_deref())
        || !valid_optional_time(value.settled_at.as_deref())
        || !valid_optional_time(value.scheduled_ends_at.as_deref())
        || value.lot_count.is_some_and(|count| !(0..=MAX_JSON_SAFE_INTEGER).contains(&count))
        || if value.status == "cancelled" {
            !cancellation_fields_present
        } else {
            !cancellation_fields_absent
        }
    {
        return Err("auction");
    }

    Ok(value)
}

fn decode_pagination(value: &AdminAuctionPaginationResponse) -> Result<(), &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.total)
    {
        return Err("pagination");
    }
    Ok(())
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
