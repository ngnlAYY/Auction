//! Admin lot 列表 API 客户端。
//!
//! 只解码后端安全投影；浏览器不接触数据库内部模型或装备密钥。

use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminAuction {
    pub id: String,
    pub world_id: String,
    pub sequence: i64,
    pub status: String,
    pub state_version: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminLot {
    pub id: String,
    pub auction_id: String,
    pub equipment_id: String,
    pub submission_id: Option<String>,
    pub lot_number: i32,
    pub status: String,
    pub start_price: i64,
    pub current_price: i64,
    pub leader_user_id: Option<String>,
    pub effective_end_at: Option<String>,
    pub slow_due_at: Option<String>,
    pub slow_generation: i64,
    pub slow_authorization_id: Option<String>,
    pub state_version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub external_equipment_id: String,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_attributes: Value,
    pub equipment_collected_at: String,
    pub seller: Option<String>,
    pub submission_source_kind: Option<String>,
    pub submission_source_reference: Option<String>,
    pub mailbox_key: Option<String>,
    pub external_mail_id: Option<String>,
    pub submission_received_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminLotPagination {
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct AdminLotCapabilities {
    pub manual_intake: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct AdminLotListData {
    auction: AdminAuction,
    lots: Vec<AdminLot>,
    pagination: AdminLotPagination,
    capabilities: AdminLotCapabilities,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminLotPage {
    pub auction: AdminAuction,
    pub lots: Vec<AdminLot>,
    pub pagination: AdminLotPagination,
    pub capabilities: AdminLotCapabilities,
}

impl From<AdminLotListData> for AdminLotPage {
    fn from(data: AdminLotListData) -> Self {
        Self {
            auction: data.auction,
            lots: data.lots,
            pagination: data.pagination,
            capabilities: data.capabilities,
        }
    }
}

pub(crate) async fn fetch_admin_lots(
    auction_id: String,
    page: i64,
) -> Result<AdminLotPage, String> {
    let url =
        format!("/api/v1/admin/auctions/{auction_id}/lots?page={}&page_size=100", page.max(1));
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminLotListData> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminLotUpdateAction {
    Withdraw,
    SetStartPrice,
}

impl AdminLotUpdateAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Withdraw => "withdraw",
            Self::SetStartPrice => "set_start_price",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct AdminLotUpdateRequest {
    command_id: Uuid,
    expected_state_version: i64,
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_price: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminLotUpdateData {
    lot: AdminLot,
}

/// 提交 Admin draft lot 修改；调用组件在同一输入重试时复用 `command_id`，不在浏览器推导状态迁移。
pub(crate) async fn update_admin_lot(
    command_id: Uuid,
    lot_id: String,
    expected_state_version: i64,
    action: AdminLotUpdateAction,
    start_price: Option<i64>,
) -> Result<AdminLot, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let url = format!("/api/v1/admin/lots/{lot_id}");
    let request = Request::patch(&url)
        .header("x-csrf-token", &csrf_token)
        .json(&AdminLotUpdateRequest {
            command_id,
            expected_state_version,
            action: action.as_str(),
            start_price,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminLotUpdateData> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.lot.id != lot_id || payload.data.lot.state_version <= 0 {
        return Err("lot update response does not match command".to_owned());
    }
    Ok(payload.data.lot)
}
