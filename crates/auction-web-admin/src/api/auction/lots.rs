//! Admin lot 列表 API 客户端。
//!
//! 只解码后端安全投影；浏览器不接触数据库内部模型或装备密钥。

pub(crate) use auction_contracts::{
    AdminLotListResponse as AdminLotPage, AdminLotResponse as AdminLot,
};
use auction_contracts::{AdminLotUpdateRequest, AdminLotUpdateResponse};
use gloo_net::http::Request;
use uuid::Uuid;

use super::ApiResponse;

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
    let payload: ApiResponse<AdminLotPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
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
            action: action.as_str().to_owned(),
            start_price,
        })
        .map_err(|error| error.to_string())?;
    let response = request.send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<AdminLotUpdateResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    if payload.data.lot.id != lot_id || payload.data.lot.state_version <= 0 {
        return Err("lot update response does not match command".to_owned());
    }
    Ok(payload.data.lot)
}
