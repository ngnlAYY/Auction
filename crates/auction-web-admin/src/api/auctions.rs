//! Admin 拍卖列表 API 客户端。
//!
//! 页面只接收后端的生命周期安全投影；创建命令携带新的 `command_id`，状态与幂等仍由服务端负责。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminAuction {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub preview_scheduled_at: String,
    pub scheduled_open_at: String,
    pub preview_opened_at: Option<String>,
    pub preview_manual_open: bool,
    pub preview_revision: i64,
    pub opened_at: Option<String>,
    pub final_phase_at: Option<String>,
    pub settled_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub cancelled_reason: Option<String>,
    pub cancelled_by: Option<String>,
    pub state_version: i64,
    pub scheduled_end_at: Option<String>,
    pub effective_end_at: Option<String>,
    pub scheduled_ends_at: Option<String>,
    pub lot_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminAuctionCapabilities {
    pub cancel_auction: bool,
    pub manual_early_open: bool,
    pub manual_intake: bool,
    pub manual_preview_open: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminAuctionPagination {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminAuctionListData {
    auctions: Vec<AdminAuction>,
    capabilities: AdminAuctionCapabilities,
    pagination: AdminAuctionPagination,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminAuctionPage {
    pub auctions: Vec<AdminAuction>,
    pub capabilities: AdminAuctionCapabilities,
    pub pagination: AdminAuctionPagination,
}

impl From<AdminAuctionListData> for AdminAuctionPage {
    fn from(data: AdminAuctionListData) -> Self {
        Self {
            auctions: data.auctions,
            capabilities: data.capabilities,
            pagination: data.pagination,
        }
    }
}

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
    let payload: ApiResponse<AdminAuctionListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

#[derive(Debug, Clone, Serialize)]
struct AdminAuctionCreateRequest {
    command_id: Uuid,
    auction_id: Uuid,
    world_id: String,
    sequence: i64,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_end_at: String,
    expected_state_version: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminAuctionCreateData {
    auction: AdminAuction,
}

/// 提交 Admin draft 创建命令；datetime-local 输入按 UTC 解释并补齐 RFC3339 秒和时区。
pub(crate) async fn create_admin_auction(
    world_id: String,
    sequence: i64,
    preview_scheduled_at: String,
    scheduled_open_at: String,
    scheduled_end_at: String,
) -> Result<AdminAuction, String> {
    let command_id = Uuid::new_v4();
    let auction_id = Uuid::new_v4();
    let csrf_token = super::fetch_csrf_token().await?;
    let request = Request::post("/api/v1/admin/auctions")
        .header("x-csrf-token", &csrf_token)
        .json(&AdminAuctionCreateRequest {
            command_id,
            auction_id,
            world_id,
            sequence,
            preview_scheduled_at: normalize_datetime(preview_scheduled_at),
            scheduled_open_at: normalize_datetime(scheduled_open_at),
            scheduled_end_at: normalize_datetime(scheduled_end_at),
            expected_state_version: 0,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminAuctionCreateData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.auction.id != auction_id.to_string() || payload.data.auction.status != "draft" {
        return Err("create response does not match command".to_owned());
    }
    Ok(payload.data.auction)
}

fn normalize_datetime(value: String) -> String {
    if value.ends_with('Z') || value.contains('+') {
        value
    } else if value.len() == 16 {
        format!("{value}:00Z")
    } else {
        format!("{value}Z")
    }
}
