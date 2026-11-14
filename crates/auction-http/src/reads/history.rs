//! 公开竞价日志与价格历史的处理器，以及价格历史筛选参数的严格解析。

use auction_application::auction::{
    MAX_PUBLIC_BID_LOG_OFFSET, MAX_PUBLIC_HISTORY_NAME_TERMS, MAX_PUBLIC_PRICE_HISTORY_OFFSET,
    PublicBidLogListRequest, PublicPriceHistoryListRequest, PublicPriceHistorySort,
};
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_contracts::{ApiErrorResponse, ApiResponse};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};

use super::mapping::{map_bid_log_page, map_price_history_page};
use crate::diagnostics;
use crate::errors::{api_error, not_found_error, query_rejection};
use crate::mapping::{decode_public_bid_log_cursor, decode_public_price_history_cursor};
use crate::middleware::response_cache::{LOW_FREQUENCY_CACHE_CONTROL, SETTLED_CACHE_CONTROL};
use crate::query::{page_offset, parse_positive_i64, parse_public_uuid, parse_query};
use crate::state::PublicApiState;

/// 公开竞价日志。编号页与 keyset cursor 页沿用旧系统互斥的分页字段。
pub(crate) async fn list_bid_log(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = parse_public_uuid(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let mut query = parse_query(&uri, &["world", "lot", "page", "page_size", "cursor"])
        .map_err(|()| query_rejection())?;
    let world_id = match query.remove("world") {
        None => None,
        Some(world) if world.is_empty() || world.len() > 64 || world != world.trim() => {
            return Err(query_rejection());
        }
        Some(world) => Some(world),
    };
    let (lot_id, lot_code) = match query.remove("lot") {
        None => (None, None),
        Some(lot) if lot.is_empty() => return Err(query_rejection()),
        Some(lot) => match parse_public_uuid(&lot) {
            Ok(lot_id) => (Some(lot_id), None),
            Err(()) if lot.len() <= 32 && lot.bytes().all(|byte| byte.is_ascii_alphanumeric()) => {
                (None, Some(lot))
            }
            Err(()) => return Err(query_rejection()),
        },
    };
    let page = parse_positive_i64(query.get("page"), 1, None).map_err(|()| query_rejection())?;
    let page_size =
        parse_positive_i64(query.get("page_size"), 20, None).map_err(|()| query_rejection())?;
    let cursor_raw = query.remove("cursor");
    let cursor = cursor_raw
        .as_deref()
        .map(decode_public_bid_log_cursor)
        .transpose()
        .map_err(|()| query_rejection())?;
    if cursor.is_some() && page != 1 {
        return Err(query_rejection());
    }
    let offset = if cursor.is_some() {
        0
    } else {
        page_offset(page, page_size, Some(MAX_PUBLIC_BID_LOG_OFFSET)).ok_or_else(query_rejection)?
    };
    let request = PublicBidLogListRequest {
        auction_id,
        world_id,
        lot_id,
        lot_code,
        limit: page_size,
        offset,
        cursor,
    };
    request.validate().map_err(|_| query_rejection())?;

    let Some(reader) = state.bid_log() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开竞价日志暂不可用",
        ));
    };
    let page_result = reader.list_public_bid_log(request).await.map_err(|diagnostic_cause| {
        diagnostics::warn(
            "public_history",
            "read_bid_log",
            DiagnosticError::diagnostic_kind(&diagnostic_cause),
            DiagnosticContext::default(),
        );

        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "公开竞价日志暂不可用")
    })?;
    let Some(page_result) = page_result else {
        return Err(not_found_error());
    };
    if page_result.limit != page_size
        || page_result.offset != offset
        || cursor_raw.is_some() != page_result.total.is_none()
        || cursor_raw.is_none() && (page_result.next_cursor.is_some() || page_result.has_more)
        || page_result.entries.iter().any(|entry| {
            entry.auction_id != auction_id || lot_id.is_some_and(|lot_id| entry.lot_id != lot_id)
        })
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开竞价日志暂不可用",
        ));
    }
    let settled = page_result.auction_status == "settled";
    let response = map_bid_log_page(page_result, page, cursor_raw);
    let mut response = Json(ApiResponse::ok("竞价日志已加载", response)).into_response();
    if settled {
        // 已结算场次的竞价事实与金额不再变化，允许长期缓存；出价人名取当前用户名，改名后
        // 按缓存档位滞后更新（见 docs/contracts/routes.md）。进行中场次仍走 2 秒目录缓存。
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(SETTLED_CACHE_CONTROL));
    }
    Ok(response)
}

/// 公开成交价格历史。过滤器和分页边界保持旧 Public 契约，cursor 页不计算统计聚合。
pub(crate) async fn list_price_history(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    let mut query = parse_query(
        &uri,
        &[
            "name",
            "world",
            "from",
            "to",
            "min_price",
            "max_price",
            "buyer",
            "seller",
            "sort",
            "page",
            "page_size",
            "cursor",
        ],
    )
    .map_err(|()| query_rejection())?;
    let name_terms = parse_history_name_terms(query.get("name")).map_err(|()| query_rejection())?;
    let world_id =
        parse_optional_history_text(query.remove("world"), 64).map_err(|()| query_rejection())?;
    let closed_from =
        parse_history_date(query.get("from"), false).map_err(|()| query_rejection())?;
    let closed_before =
        parse_history_date(query.get("to"), true).map_err(|()| query_rejection())?;
    let minimum_price =
        parse_history_price(query.get("min_price")).map_err(|()| query_rejection())?;
    let maximum_price =
        parse_history_price(query.get("max_price")).map_err(|()| query_rejection())?;
    let buyer =
        parse_optional_history_text(query.remove("buyer"), 256).map_err(|()| query_rejection())?;
    let seller =
        parse_optional_history_text(query.remove("seller"), 256).map_err(|()| query_rejection())?;
    let sort = match query.get("sort").map(String::as_str) {
        None | Some("closed_desc") => PublicPriceHistorySort::ClosedDesc,
        Some("closed_asc") => PublicPriceHistorySort::ClosedAsc,
        Some("price_desc") => PublicPriceHistorySort::PriceDesc,
        Some("price_asc") => PublicPriceHistorySort::PriceAsc,
        Some(_) => return Err(query_rejection()),
    };
    let page = parse_positive_i64(query.get("page"), 1, None).map_err(|()| query_rejection())?;
    let page_size =
        parse_positive_i64(query.get("page_size"), 50, None).map_err(|()| query_rejection())?;
    let cursor_raw = query.remove("cursor");
    let cursor = cursor_raw
        .as_deref()
        .map(decode_public_price_history_cursor)
        .transpose()
        .map_err(|()| query_rejection())?;
    // v1/type5 游标绑定旧时间倒序；不能拿同一边界解释成价格或时间升序。
    if cursor.is_some() && (page != 1 || sort != PublicPriceHistorySort::ClosedDesc) {
        return Err(query_rejection());
    }
    let offset = if cursor.is_some() {
        0
    } else {
        page_offset(page, page_size, Some(MAX_PUBLIC_PRICE_HISTORY_OFFSET))
            .ok_or_else(query_rejection)?
    };
    let request = PublicPriceHistoryListRequest {
        name_terms,
        world_id,
        closed_from,
        closed_before,
        minimum_price,
        maximum_price,
        buyer,
        seller,
        sort,
        limit: page_size,
        offset,
        cursor,
    };
    request.validate().map_err(|_| query_rejection())?;

    let Some(reader) = state.price_history() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开价格历史暂不可用",
        ));
    };
    // 读端口只有数据库实现，任何读取失败都按依赖不可用折叠；折叠前先投影安全分类。
    let page_result =
        reader.list_public_price_history(request).await.map_err(|diagnostic_cause| {
            diagnostics::warn(
                "public_history",
                "read_price_history",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "公开价格历史暂不可用",
            )
        })?;
    if page_result.limit != page_size
        || page_result.offset != offset
        || cursor_raw.is_some() != page_result.total.is_none()
        || cursor_raw.is_some() != page_result.stats.is_none()
        || cursor_raw.is_none() && (page_result.next_cursor.is_some() || page_result.has_more)
        || page_result.stats.as_ref().is_some_and(|stats| stats.validate().is_err())
        || page_result.entries.iter().any(|entry| {
            entry.lot_id.is_nil()
                || entry.auction_id.is_nil()
                || entry.world_id.is_empty()
                || entry.equipment_name.is_empty()
                || entry.equipment_category.is_empty()
                || entry.buyer_name.is_empty()
                || entry.seller_name.is_empty()
        })
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "公开价格历史暂不可用",
        ));
    }
    let response = map_price_history_page(page_result, page, cursor_raw);
    let mut response = Json(ApiResponse::ok("价格历史已加载", response)).into_response();
    // 成交历史只会追加新结算记录；低频档兼顾命中率与新记录可见性。
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(LOW_FREQUENCY_CACHE_CONTROL));
    Ok(response)
}

fn parse_history_name_terms(value: Option<&String>) -> Result<Vec<String>, ()> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if value.len() > 1024 {
        return Err(());
    }
    // 空白缩写与旧逗号参数共同拆成 AND 词条；包含匹配及 SQL 通配符转义仍由存储负责。
    let terms = value
        .split(|character: char| character == ',' || character.is_whitespace())
        .map(str::trim)
        .filter(|term| !term.is_empty())
        // 第九个非空词已足够判定超限，不再为剩余无效输入分配字符串。
        .take(MAX_PUBLIC_HISTORY_NAME_TERMS + 1)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if terms.len() > MAX_PUBLIC_HISTORY_NAME_TERMS {
        return Err(());
    }
    Ok(terms)
}

fn parse_optional_history_text(
    value: Option<String>,
    maximum: usize,
) -> Result<Option<String>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > maximum || value.chars().any(char::is_control) {
        return Err(());
    }
    Ok(Some(value))
}

fn parse_history_date(
    value: Option<&String>,
    exclusive_next_day: bool,
) -> Result<Option<DateTime<Utc>>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value != value.trim() {
        return Err(());
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| ())?;
    if date.format("%Y-%m-%d").to_string() != *value {
        return Err(());
    }
    let midnight = date.and_hms_opt(0, 0, 0).ok_or(())?;
    let timestamp = Utc.from_utc_datetime(&midnight);
    if exclusive_next_day {
        timestamp.checked_add_signed(Duration::days(1)).map(Some).ok_or(())
    } else {
        Ok(Some(timestamp))
    }
}

fn parse_history_price(value: Option<&String>) -> Result<Option<i64>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(());
    }
    value.parse::<i64>().map(Some).map_err(|_| ())
}
