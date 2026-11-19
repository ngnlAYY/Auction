//! Admin 拍卖列表 API 客户端。
//! 页面只接收后端的生命周期安全投影；创建命令携带新的 `command_id`，状态与幂等仍由服务端负责。

pub(crate) use auction_contracts::{
    AdminAuctionCapabilitiesResponse as AdminAuctionCapabilities,
    AdminAuctionListResponse as AdminAuctionPage, AdminAuctionResponse as AdminAuction,
};
use auction_contracts::{
    AdminAuctionCommandResponse, AdminAuctionLifecycleResponse, AdminCreateAuctionRequest,
    AdminUpdateAuctionScheduleRequest, AdminUpdateAuctionScheduleResponse, ApiResponse,
};
use auction_time::Timestamp;
use gloo_net::http::{Method, Request, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::{
    MAX_JSON_SAFE_INTEGER, invalid_request_error, invalid_response_error, parse_admin_time,
    response_error, send_get, send_request, valid_admin_time, valid_count, valid_optional_text,
    valid_optional_time, valid_trimmed_text, valid_uuid,
};

// 旧 Admin 列表和生命周期命令共用同一拍卖 decoder；结构闭集由 contracts 的
// `AdminAuctionResponse` 与 `AdminAuctionCommandResponse`（`deny_unknown_fields` 与必填
// nullable 语义）负责，本文件只保留跨字段业务谓词。

/// 读取一页场次（全部状态）；`world_id` 为空或 `all` 时不按世界筛选。
pub(crate) async fn fetch_admin_auctions(
    world_id: String,
    page: i64,
    page_size: i64,
) -> Result<AdminAuctionPage, String> {
    super::pagination::validate(page, page_size, 100, 100_000)?;
    let mut url = format!("/api/v1/admin/auctions?page={page}&page_size={page_size}");
    if !world_id.is_empty() && world_id != "all" {
        url.push_str("&world_id=");
        url.push_str(&world_id);
    }
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminAuctionPage> =
        response.json().await.map_err(invalid_response_error)?;
    let result = decode_auction_list(payload.data).map_err(invalid_response_error)?;
    let pagination = &result.pagination;
    if !super::pagination::valid_page_echo(
        page,
        page_size,
        pagination.page,
        pagination.page_size,
        pagination.total,
        result.auctions.len(),
    ) {
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
    preview_scheduled_at: Timestamp,
    scheduled_open_at: Timestamp,
    scheduled_end_at: Timestamp,
) -> Result<AdminAuctionCommandResponse, String> {
    let csrf_token = super::fetch_csrf_token().await?;
    let command = AdminCreateAuctionRequest {
        command_id: command_id.to_string(),
        auction_id: auction_id.to_string(),
        world_id,
        sequence,
        preview_scheduled_at: Some(preview_scheduled_at),
        scheduled_open_at: Some(scheduled_open_at),
        scheduled_end_at: Some(scheduled_end_at),
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
    let auction =
        decode_auction_command_response(response.json().await.map_err(invalid_response_error)?)?;
    if auction.id != auction_id.to_string() || auction.status != "draft" {
        return Err("invalid_response".to_owned());
    }
    Ok(auction)
}

/// 修改 Draft/Preview 拍卖的三项计划时间；同一输入重试时由组件复用命令 ID。
pub(crate) async fn update_admin_auction_schedule(
    command_id: Uuid,
    auction_id: String,
    preview_scheduled_at: Timestamp,
    scheduled_open_at: Timestamp,
    scheduled_end_at: Timestamp,
    expected_state_version: i64,
) -> Result<AdminAuction, String> {
    if !valid_uuid(&auction_id) || expected_state_version <= 0 {
        return Err("invalid_request".to_owned());
    }
    let expected_preview = preview_scheduled_at.timestamp_millis();
    let expected_open = scheduled_open_at.timestamp_millis();
    let expected_end = scheduled_end_at.timestamp_millis();
    let command = AdminUpdateAuctionScheduleRequest {
        command_id: command_id.to_string(),
        preview_scheduled_at: Some(preview_scheduled_at),
        scheduled_open_at: Some(scheduled_open_at),
        scheduled_end_at: Some(scheduled_end_at),
        expected_state_version,
    };
    let url = format!("/api/v1/admin/auctions/{auction_id}/schedule");
    let payload: ApiResponse<AdminUpdateAuctionScheduleResponse> =
        send_auction_edit(Method::PATCH, &url, &command).await?;
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
pub(super) fn decode_auction_command_response(
    payload: ApiResponse<AdminAuctionLifecycleResponse>,
) -> Result<AdminAuctionCommandResponse, String> {
    decode_auction(command_into_auction(payload.data.auction))
        .map(auction_into_command)
        .map_err(invalid_response_error)
}

/// 场次与拍品的可变命令共用写入边界；请求替身读取实际要发送的 JSON 字节。
pub(super) async fn send_auction_edit<T: DeserializeOwned>(
    method: Method,
    url: &str,
    command: &impl Serialize,
) -> Result<T, String> {
    let body = serde_json::to_string(command).map_err(invalid_request_error)?;
    #[cfg(test)]
    if let Some(stub) = dioxus::prelude::try_consume_context::<AuctionEditResponseStub>() {
        let (status, body) =
            stub.0(method, url, body.as_bytes()).map_err(|error| error.code().to_owned())?;
        if (200..300).contains(&status) {
            return serde_json::from_str(&body).map_err(invalid_response_error);
        }
        return Err(super::AdminApiFailure::from_http_body(status, &body).code);
    }

    let csrf_token = super::fetch_csrf_token().await?;
    let response = send_request(|abort_signal| {
        RequestBuilder::new(url)
            .method(method)
            .abort_signal(abort_signal)
            .header("x-csrf-token", &csrf_token)
            .header("Content-Type", "application/json")
            .body(body)
            .map_err(invalid_request_error)
    })
    .await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    response.json().await.map_err(invalid_response_error)
}

#[cfg(test)]
type AuctionEditResponder =
    dyn Fn(Method, &str, &[u8]) -> Result<(u16, String), auction_web_shared::TransportError>;

#[cfg(test)]
#[derive(Clone)]
struct AuctionEditResponseStub(std::rc::Rc<AuctionEditResponder>);

#[cfg(test)]
impl super::AdminApiFailure {
    /// 宿主回归用真实序列化请求匹配脚本，将原始状态与正文交给生产错误解码。
    pub(crate) fn stub_auction_edit(
        dispatch: impl Fn(
            Method,
            &str,
            &[u8],
        ) -> Result<(u16, String), auction_web_shared::TransportError>
        + 'static,
    ) {
        dioxus::prelude::provide_context(AuctionEditResponseStub(std::rc::Rc::new(dispatch)));
    }
}

fn decode_auction_list(data: AdminAuctionPage) -> Result<AdminAuctionPage, &'static str> {
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
        || value.lot_count.is_some_and(|count| !valid_count(count))
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

#[cfg(test)]
mod tests {

    #[test]
    fn auction_contract_fixtures() {
        use super::{
            AdminAuctionLifecycleResponse, AdminAuctionPage, ApiResponse,
            decode_auction_command_response, decode_auction_list,
        };
        use crate::api::contract_fixtures::{admin_fixture, decode};
        use crate::api::pagination::valid_page_echo;

        for (body, page, size, count) in [
            (admin_fixture!("auctions"), 1, 50, 1),
            (admin_fixture!("auctions-empty-page"), 2, 1, 0),
        ] {
            let payload: ApiResponse<AdminAuctionPage> = decode(body, "/data/pagination", "total");
            let result = decode_auction_list(payload.data).expect("场次列表");
            let pager = result.pagination;
            assert!(valid_page_echo(
                page,
                size,
                pager.page,
                pager.page_size,
                pager.total,
                result.auctions.len()
            ));
            assert_eq!(result.auctions.len(), count);
            if let Some(auction) = result.auctions.first() {
                assert_eq!(auction.status, "draft");
                assert!(auction.opened_at.is_none());
            }
        }
        let payload: ApiResponse<AdminAuctionLifecycleResponse> =
            decode(admin_fixture!("auction-create-command"), "/data/auction", "state_version");
        assert_eq!(payload.code, "created");
        let result = decode_auction_command_response(payload).expect("创建场次回执");
        assert_eq!((result.status.as_str(), result.state_version), ("draft", 1));
    }

    #[test]
    fn schedule_command_contract_fixture() {
        use auction_contracts::{AdminUpdateAuctionScheduleResponse, ApiResponse};

        use super::decode_auction;
        use crate::api::contract_fixtures::{admin_fixture, decode};

        let payload: ApiResponse<AdminUpdateAuctionScheduleResponse> =
            decode(admin_fixture!("auction-schedule-command"), "/data/auction", "state_version");
        let auction = decode_auction(payload.data.auction).expect("计划修改回执");
        assert_eq!(auction.state_version, 2);
        assert_eq!(auction.scheduled_ends_at.as_deref(), Some("2025-11-15T13:46:40.110000Z"));
    }
}
