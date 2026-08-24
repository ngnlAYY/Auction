//! Admin 拍卖列表 API 客户端。
//!
//! 页面只接收后端的生命周期安全投影；创建命令携带新的 `command_id`，状态与幂等仍由服务端负责。

pub(crate) use auction_contracts::{
    AdminAuctionCapabilitiesResponse as AdminAuctionCapabilities,
    AdminAuctionListResponse as AdminAuctionPage, AdminAuctionResponse as AdminAuction,
};
use auction_contracts::{AdminAuctionCreateResponse, AdminCreateAuctionRequest};
use chrono::{DateTime, Utc};
use gloo_net::http::Request;
use uuid::Uuid;

use super::ApiResponse;

pub(crate) async fn fetch_admin_auctions(
    world_id: String,
    status: String,
    page: i64,
) -> Result<AdminAuctionPage, String> {
    let mut url = format!("/api/v1/admin/auctions?page={page}&page_size=50");
    if world_id != "all" {
        url.push_str("&world_id=");
        url.push_str(&world_id);
    }
    if status != "all" {
        url.push_str("&status=");
        url.push_str(&status);
    }
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminAuctionPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
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
) -> Result<AdminAuction, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::post("/api/v1/admin/auctions")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminCreateAuctionRequest {
            command_id,
            auction_id,
            world_id,
            sequence,
            preview_scheduled_at: normalize_datetime(preview_scheduled_at)?,
            scheduled_open_at: normalize_datetime(scheduled_open_at)?,
            scheduled_end_at: normalize_datetime(scheduled_end_at)?,
            expected_state_version: 0,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminAuctionCreateResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.auction.id != auction_id.to_string() || payload.data.auction.status != "draft" {
        return Err("create response does not match command".to_owned());
    }
    Ok(payload.data.auction)
}

fn normalize_datetime(value: String) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(&super::normalize_datetime(&value))
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| "invalid admin auction datetime".to_owned())
}
