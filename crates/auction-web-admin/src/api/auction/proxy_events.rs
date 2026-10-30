//! Admin 代理历史 API 客户端。
//! 浏览器只解码后端的安全代理事件投影；游标、命令账本结果和代理状态机仍由服务端拥有。

pub(crate) use auction_contracts::{
    AdminProxyEventListResponse as AdminProxyEventPage, AdminProxyEventResponse as AdminProxyEvent,
};

use super::{
    ApiResponse, MAX_JSON_SAFE_INTEGER, invalid_response_error, response_error, send_get,
    valid_admin_time, valid_trimmed_text, valid_uuid,
};

// 结构闭集由 contracts 的 `AdminProxyEventListResponse`（含 event 与混合分页，
// `deny_unknown_fields`）负责；本文件只保留跨字段谓词（usage 不超过授权序号、请求回显等）
// 以及页码模式下游标字段必须为空的组合校验。

pub(crate) async fn fetch_admin_proxy_events(
    lot_id: String,
    page: i64,
    page_size: i64,
) -> Result<AdminProxyEventPage, String> {
    super::pagination::validate(page, page_size, 100, 100_000)?;
    let url = format!("/api/v1/admin/lots/{lot_id}/proxy-events?page={page}&page_size={page_size}");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<AdminProxyEventPage> =
        response.json().await.map_err(invalid_response_error)?;
    decode_proxy_event_page(payload.data, &lot_id, page, page_size).map_err(invalid_response_error)
}

fn decode_proxy_event_page(
    data: AdminProxyEventPage,
    requested_lot_id: &str,
    requested_page: i64,
    requested_size: i64,
) -> Result<AdminProxyEventPage, &'static str> {
    let pagination = data.pagination;
    // 页码模式下游标字段必须为空，页码与总数必须显式回显。
    if !valid_uuid(requested_lot_id)
        || pagination.cursor.is_some()
        || pagination.next_cursor.is_some()
        || pagination.has_more.is_some()
        || !pagination.page.is_some_and(|page| (1..=MAX_JSON_SAFE_INTEGER).contains(&page))
        || pagination.page != Some(requested_page)
        || pagination.page_size != requested_size
        || !pagination.total.is_some_and(|total| (0..=MAX_JSON_SAFE_INTEGER).contains(&total))
        || i64::try_from(data.proxy_events.len())
            .map_or(true, |count| count > pagination.page_size || Some(count) > pagination.total)
    {
        return Err("proxy_pagination");
    }

    let proxy_events = data
        .proxy_events
        .into_iter()
        .map(|event| decode_proxy_event(event, requested_lot_id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AdminProxyEventPage { proxy_events, pagination })
}

/// 跨字段谓词保留为独立校验：usage 序号不超过授权序号、请求 lot 回显一致。
fn decode_proxy_event(
    event: AdminProxyEvent,
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

    Ok(event)
}
