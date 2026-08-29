//! Admin lot 列表 API 客户端。
//!
//! 只解码后端安全投影；浏览器不接触数据库内部模型或装备密钥。

pub(crate) use auction_contracts::AdminLotResponse as AdminLot;
use auction_contracts::{AdminAuctionResponse, AdminLotPaginationResponse, AdminLotUpdateRequest};
use gloo_net::http::Request;
use serde::Deserialize;
use uuid::Uuid;

use super::auctions::{StrictAuction, decode_auction};
use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, RequiredOption, TABLE_PAGE_SIZE, invalid_request_error,
    invalid_response_error, response_error, send_get, send_request, valid_admin_time,
    valid_trimmed_text, valid_uuid,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLot {
    auction_id: String,
    created_at: String,
    current_price: i64,
    effective_end_at: RequiredOption<String>,
    equipment_attributes: serde_json::Value,
    equipment_category: String,
    equipment_collected_at: String,
    equipment_id: String,
    equipment_name: String,
    external_equipment_id: String,
    external_mail_id: RequiredOption<String>,
    id: String,
    leader_user_id: RequiredOption<String>,
    lot_number: i32,
    mailbox_key: RequiredOption<String>,
    seller: RequiredOption<String>,
    slow_authorization_id: RequiredOption<String>,
    slow_due_at: RequiredOption<String>,
    slow_generation: i64,
    start_price: i64,
    state_version: i64,
    status: String,
    submission_id: RequiredOption<String>,
    submission_received_at: RequiredOption<String>,
    submission_source_kind: RequiredOption<String>,
    submission_source_reference: RequiredOption<String>,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLotPagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLotList {
    auction: StrictAuction,
    lots: Vec<StrictLot>,
    pagination: StrictLotPagination,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLotData {
    lot: StrictLot,
}

/// lot 页的浏览器本地模型；响应 body 保持旧 v1 闭集，额外能力只从非破坏性响应头读取。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminLotPage {
    pub(crate) auction: AdminAuctionResponse,
    pub(crate) lots: Vec<AdminLot>,
    pub(crate) pagination: AdminLotPaginationResponse,
    pub(crate) capabilities: AdminLotCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdminLotCapabilities {
    pub(crate) manual_intake: bool,
}

pub(crate) async fn fetch_admin_lots(
    auction_id: String,
    page: i64,
) -> Result<AdminLotPage, String> {
    let url = format!(
        "/api/v1/admin/auctions/{auction_id}/lots?page={}&page_size={TABLE_PAGE_SIZE}",
        page.max(1)
    );
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let manual_intake =
        response.headers().get("x-auction-capability-manual-intake").as_deref() == Some("true");
    let payload: ApiResponse<StrictLotList> =
        response.json().await.map_err(invalid_response_error)?;
    let auction = decode_auction(payload.data.auction).map_err(invalid_response_error)?;
    let lots = payload
        .data
        .lots
        .into_iter()
        .map(decode_lot)
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid_response_error)?;
    if lots.iter().any(|lot| lot.auction_id != auction.id) {
        return Err("invalid_response".to_owned());
    }
    let pagination = decode_pagination(payload.data.pagination).map_err(invalid_response_error)?;
    Ok(AdminLotPage {
        auction,
        lots,
        pagination,
        capabilities: AdminLotCapabilities { manual_intake },
    })
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
    let command = AdminLotUpdateRequest {
        command_id,
        expected_state_version,
        action: action.as_str().to_owned(),
        start_price,
    };
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
    let payload: ApiResponse<StrictLotData> =
        response.json().await.map_err(invalid_response_error)?;
    let lot = decode_lot(payload.data.lot).map_err(invalid_response_error)?;
    if lot.id != lot_id || lot.state_version <= 0 {
        return Err("invalid_response".to_owned());
    }
    Ok(lot)
}

fn decode_lot(value: StrictLot) -> Result<AdminLot, &'static str> {
    if !valid_uuid(&value.id)
        || !valid_uuid(&value.auction_id)
        || !valid_uuid(&value.equipment_id)
        || !valid_optional_uuid(value.submission_id.0.as_deref())
        || value.lot_number <= 0
        || !matches!(value.status.as_str(), "pending" | "live" | "sold" | "unsold" | "withdrawn")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.start_price)
        || !(value.start_price..=MAX_JSON_SAFE_INTEGER).contains(&value.current_price)
        || !valid_optional_uuid(value.leader_user_id.0.as_deref())
        || !valid_optional_time(value.effective_end_at.0.as_deref())
        || !valid_optional_time(value.slow_due_at.0.as_deref())
        || !valid_optional_uuid(value.slow_authorization_id.0.as_deref())
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.slow_generation)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.state_version)
        || !valid_admin_time(&value.created_at)
        || !valid_admin_time(&value.updated_at)
        || !valid_trimmed_text(&value.external_equipment_id)
        || !valid_trimmed_text(&value.equipment_name)
        || !valid_trimmed_text(&value.equipment_category)
        || !value.equipment_attributes.is_object()
        || !valid_admin_time(&value.equipment_collected_at)
        || !valid_optional_text(value.seller.0.as_deref())
        || !valid_optional_text(value.mailbox_key.0.as_deref())
        || !valid_optional_text(value.external_mail_id.0.as_deref())
        || !valid_optional_time(value.submission_received_at.0.as_deref())
        || !value
            .submission_source_kind
            .0
            .as_deref()
            .is_none_or(|kind| matches!(kind, "admin" | "mail"))
        || !valid_optional_text(value.submission_source_reference.0.as_deref())
        || matches!(value.status.as_str(), "live" | "sold") && value.effective_end_at.0.is_none()
        || (value.slow_due_at.0.is_none() != value.slow_authorization_id.0.is_none())
        || value.slow_due_at.0.is_some() && (value.status != "live" || value.slow_generation <= 0)
        || !valid_submission_fields(&value)
    {
        return Err("lot");
    }

    Ok(AdminLot {
        id: value.id,
        auction_id: value.auction_id,
        equipment_id: value.equipment_id,
        submission_id: value.submission_id.0,
        lot_number: value.lot_number,
        status: value.status,
        start_price: value.start_price,
        current_price: value.current_price,
        leader_user_id: value.leader_user_id.0,
        effective_end_at: value.effective_end_at.0,
        slow_due_at: value.slow_due_at.0,
        slow_generation: value.slow_generation,
        slow_authorization_id: value.slow_authorization_id.0,
        state_version: value.state_version,
        created_at: value.created_at,
        updated_at: value.updated_at,
        external_equipment_id: value.external_equipment_id,
        equipment_name: value.equipment_name,
        equipment_category: value.equipment_category,
        equipment_attributes: value.equipment_attributes,
        equipment_collected_at: value.equipment_collected_at,
        seller: value.seller.0,
        submission_source_kind: value.submission_source_kind.0,
        submission_source_reference: value.submission_source_reference.0,
        mailbox_key: value.mailbox_key.0,
        external_mail_id: value.external_mail_id.0,
        submission_received_at: value.submission_received_at.0,
    })
}

fn valid_submission_fields(value: &StrictLot) -> bool {
    if value.submission_id.0.is_none() {
        return [
            value.seller.0.as_ref(),
            value.submission_source_kind.0.as_ref(),
            value.submission_source_reference.0.as_ref(),
            value.mailbox_key.0.as_ref(),
            value.external_mail_id.0.as_ref(),
            value.submission_received_at.0.as_ref(),
        ]
        .into_iter()
        .all(|item| item.is_none());
    }
    if value.seller.0.is_none()
        || value.submission_source_kind.0.is_none()
        || value.submission_received_at.0.is_none()
    {
        return false;
    }
    if value.submission_source_kind.0.as_deref() == Some("mail") {
        value.submission_source_reference.0.is_none()
            && value.mailbox_key.0.is_some()
            && value.external_mail_id.0.is_some()
    } else {
        value.submission_source_reference.0.is_some()
            && value.mailbox_key.0.is_none()
            && value.external_mail_id.0.is_none()
    }
}

fn decode_pagination(
    value: StrictLotPagination,
) -> Result<AdminLotPaginationResponse, &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.total)
    {
        return Err("pagination");
    }
    Ok(AdminLotPaginationResponse {
        page: value.page,
        page_size: value.page_size,
        total: value.total,
    })
}

fn valid_optional_uuid(value: Option<&str>) -> bool {
    value.is_none_or(valid_uuid)
}

fn valid_optional_time(value: Option<&str>) -> bool {
    value.is_none_or(valid_admin_time)
}

fn valid_optional_text(value: Option<&str>) -> bool {
    value.is_none_or(valid_trimmed_text)
}
