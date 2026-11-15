//! Public 持久化实时事件列表读取。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::realtime::PublicRealtimeRequest;
use auction_contracts::{ApiErrorResponse, ApiResponse};
use axum::{
    Json,
    extract::State,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::mapping::{map_auction_event_page, valid_page};
use super::target::{LIST_QUERY_KEYS, parse_http_target, parse_nonnegative, parse_page_size};
use crate::diagnostics;
use crate::errors::{api_error, not_found_error, query_rejection};
use crate::query::parse_query;
use crate::state::PublicApiState;

/// 读取指定拍卖流的持久化事件。
pub(crate) async fn list(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    let query = parse_query(&uri, LIST_QUERY_KEYS).map_err(|()| query_rejection())?;
    let auction_id = parse_http_target(&query).map_err(|()| query_rejection())?;
    let after = parse_nonnegative(query.get("after"), 0).map_err(|()| query_rejection())?;
    let page_size = parse_page_size(query.get("page_size")).map_err(|()| query_rejection())?;
    let request = PublicRealtimeRequest { auction_id, after, page_size };
    request.validate().map_err(|_| query_rejection())?;

    let Some(reader) = state.realtime() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "实时事件暂不可用",
        ));
    };
    let page = reader.list_public_events(request).await.map_err(|diagnostic_cause| {
        diagnostics::warn(
            "public_realtime",
            "read_events",
            DiagnosticError::diagnostic_kind(&diagnostic_cause),
            DiagnosticContext::default(),
        );

        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "实时事件暂不可用")
    })?;
    let Some(page) = page else {
        return Err(not_found_error());
    };
    if !valid_page(&page, auction_id, after)
        || i64::try_from(page.events.len()).map_or(true, |count| count > page_size)
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "实时事件暂不可用",
        ));
    }

    Ok(Json(ApiResponse::ok("实时事件已加载", map_auction_event_page(page, auction_id)))
        .into_response())
}
