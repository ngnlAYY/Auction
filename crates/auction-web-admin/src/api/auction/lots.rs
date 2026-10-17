//! Admin lot 列表 API 客户端。
//! 只解码后端安全投影；浏览器不接触数据库内部模型或装备密钥。

pub(crate) use auction_contracts::AdminLotResponse as AdminLot;
use auction_contracts::{
    AdminAuctionResponse, AdminLotListResponse, AdminLotPaginationResponse, AdminLotUpdateRequest,
    AdminLotUpdateResponse,
};
use gloo_net::http::Request;
use uuid::Uuid;

use super::auctions::decode_auction;
use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, invalid_request_error, invalid_response_error,
    response_error, send_get, send_request, valid_admin_time, valid_trimmed_text, valid_uuid,
};

// 本地 Strict 镜像已删除：contracts 的 `AdminLotResponse`/`AdminLotListResponse`/
// `AdminLotUpdateResponse` 已带 `deny_unknown_fields` 与必填 nullable 语义；本文件只保留
// 跨字段业务谓词（slow 字段组合、submission 字段组合等）。

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
    page_size: i64,
) -> Result<AdminLotPage, String> {
    super::pagination::validate(page, page_size, 100, 100_000)?;
    let url = format!("/api/v1/admin/auctions/{auction_id}/lots?page={page}&page_size={page_size}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let manual_intake =
        response.headers().get("x-auction-capability-manual-intake").as_deref() == Some("true");
    let payload: ApiResponse<AdminLotListResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let auction = decode_auction(payload.data.auction).map_err(invalid_response_error)?;
    let lots = payload
        .data
        .lots
        .into_iter()
        .map(decode_lot)
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid_response_error)?;
    if auction.id != auction_id || lots.iter().any(|lot| lot.auction_id != auction.id) {
        return Err("invalid_response".to_owned());
    }
    let pagination = payload.data.pagination;
    decode_pagination(&pagination).map_err(invalid_response_error)?;
    if pagination.page != page
        || pagination.page_size != page_size
        || i64::try_from(lots.len())
            .map_or(true, |count| count > page_size || count > pagination.total)
    {
        return Err(invalid_response_error("lot_pagination"));
    }
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
    let payload: ApiResponse<AdminLotUpdateResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let lot = decode_lot(payload.data.lot).map_err(invalid_response_error)?;
    if lot.id != lot_id || lot.state_version <= 0 {
        return Err("invalid_response".to_owned());
    }
    Ok(lot)
}

/// 跨字段谓词保留为独立校验：slow/提交字段组合与值域仍在浏览器边界 fail-closed。
fn decode_lot(value: AdminLot) -> Result<AdminLot, &'static str> {
    if !valid_uuid(&value.id)
        || !valid_uuid(&value.auction_id)
        || !valid_uuid(&value.equipment_id)
        || !valid_optional_uuid(value.submission_id.as_deref())
        || value.lot_number <= 0
        || !matches!(value.status.as_str(), "pending" | "live" | "sold" | "unsold" | "withdrawn")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.start_price)
        || !(value.start_price..=MAX_JSON_SAFE_INTEGER).contains(&value.current_price)
        || !valid_optional_uuid(value.leader_user_id.as_deref())
        || !valid_optional_time(value.effective_end_at.as_deref())
        || !valid_optional_time(value.slow_due_at.as_deref())
        || !valid_optional_uuid(value.slow_authorization_id.as_deref())
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.slow_generation)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.state_version)
        || !valid_admin_time(&value.created_at)
        || !valid_admin_time(&value.updated_at)
        || !valid_trimmed_text(&value.external_equipment_id)
        || !valid_trimmed_text(&value.equipment_name)
        || !valid_trimmed_text(&value.equipment_category)
        || !value.equipment_attributes.is_object()
        || !valid_admin_time(&value.equipment_collected_at)
        || !valid_optional_text(value.seller.as_deref())
        || !valid_optional_text(value.mailbox_key.as_deref())
        || !valid_optional_text(value.external_mail_id.as_deref())
        || !valid_optional_time(value.submission_received_at.as_deref())
        || !value
            .submission_source_kind
            .as_deref()
            .is_none_or(|kind| matches!(kind, "admin" | "mail"))
        || !valid_optional_text(value.submission_source_reference.as_deref())
        || matches!(value.status.as_str(), "live" | "sold") && value.effective_end_at.is_none()
        || (value.slow_due_at.is_none() != value.slow_authorization_id.is_none())
        || value.slow_due_at.is_some() && (value.status != "live" || value.slow_generation <= 0)
        || !valid_submission_fields(&value)
    {
        return Err("lot");
    }

    Ok(value)
}

fn valid_submission_fields(value: &AdminLot) -> bool {
    if value.submission_id.is_none() {
        return [
            value.seller.as_ref(),
            value.submission_source_kind.as_ref(),
            value.submission_source_reference.as_ref(),
            value.mailbox_key.as_ref(),
            value.external_mail_id.as_ref(),
            value.submission_received_at.as_ref(),
        ]
        .into_iter()
        .all(|item| item.is_none());
    }
    if value.seller.is_none()
        || value.submission_source_kind.is_none()
        || value.submission_received_at.is_none()
    {
        return false;
    }
    if value.submission_source_kind.as_deref() == Some("mail") {
        value.submission_source_reference.is_none()
            && value.mailbox_key.is_some()
            && value.external_mail_id.is_some()
    } else {
        value.submission_source_reference.is_some()
            && value.mailbox_key.is_none()
            && value.external_mail_id.is_none()
    }
}

fn decode_pagination(value: &AdminLotPaginationResponse) -> Result<(), &'static str> {
    if !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&value.page_size)
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&value.total)
    {
        return Err("pagination");
    }
    Ok(())
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
