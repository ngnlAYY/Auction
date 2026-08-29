//! Admin 代理历史 API 客户端。
//!
//! 浏览器只解码后端的安全代理事件投影；游标、命令账本结果和代理状态机仍由服务端拥有。

use auction_contracts::AdminProxyEventPaginationResponse;
pub(crate) use auction_contracts::{
    AdminProxyEventListResponse as AdminProxyEventPage, AdminProxyEventResponse as AdminProxyEvent,
};
use serde::Deserialize;

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, TABLE_PAGE_SIZE, invalid_response_error, response_error,
    send_get, valid_admin_time, valid_trimmed_text, valid_uuid,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictProxyEvent {
    authorization_id: String,
    authorization_sequence: i64,
    command_id: String,
    created_at: String,
    lot_id: String,
    maximum_amount: i64,
    mode: String,
    state_version: i64,
    status: String,
    user_id: String,
    usage_number: i64,
    username: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictProxyPagination {
    page: i64,
    page_size: i64,
    total: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictProxyEventPage {
    proxy_events: Vec<StrictProxyEvent>,
    pagination: StrictProxyPagination,
}

pub(crate) async fn fetch_admin_proxy_events(
    lot_id: String,
    page: i64,
) -> Result<AdminProxyEventPage, String> {
    let url =
        format!("/api/v1/admin/lots/{lot_id}/proxy-events?page={page}&page_size={TABLE_PAGE_SIZE}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<StrictProxyEventPage> =
        response.json().await.map_err(invalid_response_error)?;
    decode_proxy_event_page(payload.data, &lot_id, page).map_err(invalid_response_error)
}

fn decode_proxy_event_page(
    data: StrictProxyEventPage,
    requested_lot_id: &str,
    requested_page: i64,
) -> Result<AdminProxyEventPage, &'static str> {
    let pagination = data.pagination;
    if !valid_uuid(requested_lot_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&pagination.page)
        || pagination.page != requested_page
        || pagination.page_size != TABLE_PAGE_SIZE
        || !(0..=MAX_JSON_SAFE_INTEGER).contains(&pagination.total)
        || i64::try_from(data.proxy_events.len())
            .map_or(true, |count| count > pagination.page_size || count > pagination.total)
    {
        return Err("proxy_pagination");
    }

    let proxy_events = data
        .proxy_events
        .into_iter()
        .map(|event| decode_proxy_event(event, requested_lot_id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AdminProxyEventPage {
        proxy_events,
        pagination: AdminProxyEventPaginationResponse {
            page: Some(pagination.page),
            page_size: pagination.page_size,
            total: Some(pagination.total),
            cursor: None,
            next_cursor: None,
            has_more: None,
        },
    })
}

fn decode_proxy_event(
    event: StrictProxyEvent,
    requested_lot_id: &str,
) -> Result<AdminProxyEvent, &'static str> {
    if !valid_uuid(&event.command_id)
        || !valid_uuid(&event.authorization_id)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&event.authorization_sequence)
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&event.usage_number)
        || event.usage_number > event.authorization_sequence
        || !valid_uuid(&event.lot_id)
        || event.lot_id != requested_lot_id
        || !valid_uuid(&event.user_id)
        || !valid_trimmed_text(&event.username)
        || !matches!(event.mode.as_str(), "fast" | "slow")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&event.maximum_amount)
        || !matches!(event.status.as_str(), "active" | "exhausted" | "frozen" | "closed")
        || !(1..=MAX_JSON_SAFE_INTEGER).contains(&event.state_version)
        || !valid_admin_time(&event.created_at)
    {
        return Err("proxy_event");
    }

    Ok(AdminProxyEvent {
        authorization_id: event.authorization_id,
        authorization_sequence: event.authorization_sequence,
        command_id: event.command_id,
        created_at: event.created_at,
        lot_id: event.lot_id,
        maximum_amount: event.maximum_amount,
        mode: event.mode,
        state_version: event.state_version,
        status: event.status,
        user_id: event.user_id,
        usage_number: event.usage_number,
        username: event.username,
    })
}
