//! Public 持久化实时事件列表读取。

use auction_application::{MAX_PUBLIC_EVENT_PAGE_SIZE, PublicRealtimeRequest};
use auction_contracts::{ApiErrorResponse, ApiResponse, PublicRealtimeResponse};
use axum::{
    Json,
    extract::State,
    http::{StatusCode, Uri},
};

use super::super::{PublicApiState, api_error, not_found_error, parse_positive_i64, parse_query};
use super::mapping::{map_page, valid_page};

/// 读取指定拍卖流或公开发现流的持久化事件。
pub(crate) async fn list(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicRealtimeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let query = parse_query(&uri, &["auction_id", "after", "page_size"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let auction_id = query
        .get("auction_id")
        .map(|value| {
            uuid::Uuid::parse_str(value).map_err(|_| {
                api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效")
            })
        })
        .transpose()?;
    let after =
        query
            .get("after")
            .map(|value| {
                value.parse::<i64>().ok().filter(|value| *value >= 0).ok_or_else(|| {
                    api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                })
            })
            .transpose()?
            .unwrap_or(0);
    let page_size =
        parse_positive_i64(query.get("page_size"), 50, Some(MAX_PUBLIC_EVENT_PAGE_SIZE))
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
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
    if !valid_page(&page, auction_id, after) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "实时事件暂不可用",
        ));
    }

    Ok(Json(ApiResponse::ok("实时事件已加载", map_page(page))))
}
