//! Public 持久化实时事件列表读取。

use auction_application::PublicRealtimeRequest;
use auction_contracts::{ApiErrorResponse, ApiResponse};
use axum::{
    Json,
    extract::State,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::super::{PublicApiState, api_error, not_found_error, parse_query};
use super::mapping::{map_auction_page, map_discovery_page, valid_page};
use super::target::{RealtimeTarget, parse_http_target, parse_nonnegative, parse_page_size};

/// 读取指定拍卖流或公开发现流的持久化事件。
pub(crate) async fn list(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    let query = parse_query(&uri, &["auction_id", "scope", "after", "page_size"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let target = parse_http_target(&query)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let after = parse_nonnegative(query.get("after"), 0)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size = parse_page_size(query.get("page_size"))
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let auction_id = target.auction_id();
    let request = PublicRealtimeRequest { auction_id, after, page_size };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

    let Some(reader) = state.realtime() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "实时事件暂不可用",
        ));
    };
    let page = reader.list_public_events(request).await.map_err(|_| {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "实时事件暂不可用")
    })?;
    let Some(page) = page else {
        return Err(not_found_error());
    };
    if !valid_page(&page, auction_id, after)
        || i64::try_from(page.events.len()).map_or(true, |count| count > page_size)
        || matches!(target, RealtimeTarget::Discovery)
            && page.events.iter().any(|event| event.kind != "auction.created")
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "实时事件暂不可用",
        ));
    }

    Ok(match target {
        RealtimeTarget::Auction(auction_id) => {
            Json(ApiResponse::ok("实时事件已加载", map_auction_page(page, auction_id)))
                .into_response()
        }
        RealtimeTarget::Discovery => {
            Json(ApiResponse::ok("实时事件已加载", map_discovery_page(page))).into_response()
        }
    })
}
