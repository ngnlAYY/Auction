//! 公开竞价日志与价格历史的处理器，以及价格历史筛选参数的严格解析。

use auction_application::auction::{
    MAX_PUBLIC_BID_LOG_OFFSET, MAX_PUBLIC_HISTORY_NAME_TERMS, MAX_PUBLIC_PRICE_HISTORY_OFFSET,
    PublicBidLogListRequest, PublicPriceHistoryListRequest, PublicPriceHistorySort,
};
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_contracts::{ApiErrorResponse, ApiResponse};
use auction_time::{Duration, Timestamp};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use super::mapping::{map_bid_log_page, map_price_history_page};
use crate::diagnostics;
use crate::errors::{api_error, not_found_error, query_rejection};
use crate::mapping::{decode_public_bid_log_cursor, decode_public_price_history_cursor};
use crate::middleware::response_cache::{LOW_FREQUENCY_CACHE_CONTROL, SETTLED_CACHE_CONTROL};
use crate::query::{
    page_offset, parse_positive_i64, parse_public_page_size, parse_public_uuid, parse_query,
};
use crate::state::PublicApiState;

/// 公开竞价日志。编号页与 keyset cursor 页使用已发布的互斥分页字段。
///
/// 条目缺省保持原字段闭集：已打开的旧页面按闭集解码，收到未知键会整条拒绝。代理竞争中的
/// 落败授权只在 `include=defeated_proxy` 显式请求时追加。
pub(crate) async fn list_bid_log(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = parse_public_uuid(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let mut query = parse_query(&uri, &["world", "lot", "page", "page_size", "cursor", "include"])
        .map_err(|()| query_rejection())?;
    // 只接受登记的附加字段；未知值失败关闭，不静默退回原字段集合。
    let include_defeated_proxy = match query.remove("include").as_deref() {
        None => false,
        Some("defeated_proxy") => true,
        Some(_) => {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "附加字段无效"));
        }
    };
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
        parse_public_page_size(query.get("page_size"), 20).map_err(|()| query_rejection())?;
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
        || cursor_raw.is_some() && (page_result.has_more != page_result.next_cursor.is_some())
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
    let response = map_bid_log_page(page_result, page, cursor_raw, include_defeated_proxy);
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
        parse_public_page_size(query.get("page_size"), 50).map_err(|()| query_rejection())?;
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

    let filter_request = request.clone();
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
        || cursor_raw.is_some() && (page_result.has_more != page_result.next_cursor.is_some())
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
                || !price_history_entry_matches_request(entry, &filter_request)
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

fn price_history_entry_matches_request(
    entry: &auction_application::auction::PublicPriceHistoryEntry,
    request: &PublicPriceHistoryListRequest,
) -> bool {
    let name = entry.equipment_name.to_lowercase();
    request.name_terms.iter().all(|term| name.contains(&term.to_lowercase()))
        && request.world_id.as_ref().is_none_or(|world| &entry.world_id == world)
        && request.closed_from.is_none_or(|from| entry.closed_at >= from)
        && request.closed_before.is_none_or(|before| entry.closed_at < before)
        && request.minimum_price.is_none_or(|minimum| entry.price >= minimum)
        && request.maximum_price.is_none_or(|maximum| entry.price <= maximum)
        && request.buyer.as_ref().is_none_or(|buyer| entry.buyer_name.eq_ignore_ascii_case(buyer))
        && request
            .seller
            .as_ref()
            .is_none_or(|seller| entry.seller_name.eq_ignore_ascii_case(seller))
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
) -> Result<Option<Timestamp>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value != value.trim() {
        return Err(());
    }
    let date = auction_time::parse_date(value).ok_or(())?;
    if date.strftime("%Y-%m-%d").to_string() != *value {
        return Err(());
    }
    let timestamp = Timestamp::from_utc_datetime(date.at(0, 0, 0, 0)).ok_or(())?;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::auction::{
        PublicBidLogEntry, PublicBidLogListRequest, PublicBidLogPage, PublicBidLogReader,
        PublicDefeatedProxy, PublicPriceHistoryEntry, PublicPriceHistoryListRequest,
        PublicPriceHistoryPage, PublicPriceHistoryReader, PublicPriceHistoryStats,
    };
    use auction_application::core::ApplicationFuture;
    use auction_time::Timestamp;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{list_bid_log, list_price_history};
    use crate::middleware::security::PublicHttpSecurity;
    use crate::server::contract_fixtures::{assert_http, assert_wire};
    use crate::state::PublicApiState;

    const AUCTION_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b);
    const LOT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2c);

    fn entry(
        bid: u128,
        bidder_name: &str,
        amount: i64,
        state_version: i64,
        defeated_proxy: Option<PublicDefeatedProxy>,
    ) -> PublicBidLogEntry {
        PublicBidLogEntry {
            id: Uuid::from_u128(0x4000_8000_0000_0000_0000 + bid),
            auction_id: AUCTION_ID,
            lot_id: LOT_ID,
            lot_number: 1,
            amount,
            equipment_name: "Legendary Staff".to_owned(),
            equipment_category: "Staff".to_owned(),
            bidder_name: bidder_name.to_owned(),
            source: Some("proxy".to_owned()),
            source_post_number: None,
            source_thread_id: None,
            source_post_id: None,
            valid: defeated_proxy.is_some(),
            accepted_state_version: state_version,
            fact_index: 1,
            mechanism: "proxy".to_owned(),
            created_at: Timestamp::from_timestamp(1_763_124_640, 0).expect("fixture time"),
            defeated_proxy,
        }
    }

    /// 一页两条代理事实：较新的一条在代理竞争中击败了另一份授权。
    struct FixedBidLog {
        final_phase: bool,
    }

    impl PublicBidLogReader for FixedBidLog {
        fn list_public_bid_log(
            &self,
            request: PublicBidLogListRequest,
        ) -> ApplicationFuture<'_, Option<PublicBidLogPage>> {
            let final_phase = self.final_phase;
            Box::pin(async move {
                let defeated = PublicDefeatedProxy {
                    bidder_name: "challenger".to_owned(),
                    maximum_amount: 15_000_000,
                };
                let mut entries = vec![
                    entry(2, "winner", 15_750_000, 11, Some(defeated)),
                    entry(1, "winner", 10_500_000, 10, None),
                ];
                if final_phase {
                    // 同时刻由版本与事实序号打破平局；Final 之后的新版本仍保留原事实。
                    entries[0].created_at =
                        Timestamp::from_timestamp(1_763_124_640, 100_000_000).expect("出价时刻");
                    entries[1].created_at = entries[0].created_at;
                    entries[0].accepted_state_version = 13;
                    entries[1].accepted_state_version = 12;
                    // 当前领先人与价格才产生 valid，历史事实仍保留其击败的代理。
                    entries[0].valid = false;
                    let mut later = entry(3, "later", 18_000_000, 14, None);
                    later.mechanism = "ordinary".to_owned();
                    later.source = None;
                    later.created_at =
                        Timestamp::from_timestamp(1_763_124_640, 110_000_000).expect("出价时刻");
                    later.valid = true;
                    entries.insert(0, later);
                }
                Ok(Some(PublicBidLogPage {
                    auction_status: if final_phase { "final" } else { "live" }.to_owned(),
                    total: request
                        .cursor
                        .is_none()
                        .then(|| i64::try_from(entries.len()).expect("出价数量")),
                    entries,
                    limit: request.limit,
                    offset: request.offset,
                    next_cursor: None,
                    has_more: false,
                }))
            })
        }
    }

    async fn request(query: &str) -> (StatusCode, Value) {
        request_bid_log(query, false).await
    }

    async fn request_bid_log(query: &str, final_phase: bool) -> (StatusCode, Value) {
        let state = PublicApiState::new(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(FixedBidLog { final_phase })),
            None,
            None,
            None,
            None,
            None,
            PublicHttpSecurity::default(),
        );
        let app = Router::new()
            .route("/api/v1/auctions/{auction_id}/bid-log", get(list_bid_log))
            .with_state(state);
        let separator = if query.is_empty() { "" } else { "?" };
        let uri = format!("/api/v1/auctions/{AUCTION_ID}/bid-log{separator}{query}");
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        (status, serde_json::from_slice(&body).expect("json body"))
    }

    /// 回归：不带 `include` 时条目保持原字段闭集，已打开的旧页面的闭集解码照常接受。
    #[tokio::test]
    async fn bid_log_keeps_the_original_entry_fields_by_default() {
        for query in ["", "page=1&page_size=20"] {
            let (status, body) = request(query).await;
            assert_eq!(status, StatusCode::OK, "{query}");
            for bid in body["data"]["bids"].as_array().expect("bids") {
                let mut keys =
                    bid.as_object().expect("bid").keys().map(String::as_str).collect::<Vec<_>>();
                keys.sort_unstable();
                assert_eq!(
                    keys,
                    [
                        "accepted_state_version",
                        "amount",
                        "auction_id",
                        "bidder_name",
                        "created_at",
                        "equipment_category",
                        "equipment_name",
                        "fact_index",
                        "id",
                        "lot_id",
                        "lot_number",
                        "mechanism",
                        "source",
                        "valid",
                    ],
                    "{query}"
                );
            }
        }
    }

    /// `include=defeated_proxy` 只给击败了授权的那条出价追加落败代理，其余字段与缺省响应相同。
    #[tokio::test]
    async fn include_adds_only_the_defeated_proxy() {
        let (status, original) = request("page=1").await;
        assert_eq!(status, StatusCode::OK);
        let (status, included) = request("page=1&include=defeated_proxy").await;
        assert_eq!(status, StatusCode::OK);
        let mut expected = original;
        expected["data"]["bids"][0]["defeated_proxy"] =
            json!({ "bidder_name": "challenger", "maximum_amount": 15_000_000 });
        assert_eq!(included, expected);
    }

    #[tokio::test]
    async fn unknown_or_repeated_include_fails_closed() {
        for (query, message) in [
            ("include=", "附加字段无效"),
            ("include=DEFEATED_PROXY", "附加字段无效"),
            ("include=proxy_default_mode", "附加字段无效"),
            ("include=defeated_proxy,mode", "附加字段无效"),
            ("include=defeated_proxy&include=defeated_proxy", "查询参数无效"),
            ("fields=defeated_proxy", "查询参数无效"),
        ] {
            let (status, body) = request(query).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], message, "{query}");
        }
    }

    struct FixedPriceHistory;

    impl PublicPriceHistoryReader for FixedPriceHistory {
        fn list_public_price_history(
            &self,
            request: PublicPriceHistoryListRequest,
        ) -> ApplicationFuture<'_, PublicPriceHistoryPage> {
            Box::pin(async move {
                let at = Timestamp::from_timestamp(1_763_124_640, 110_000_000).expect("成交时刻");
                let row = PublicPriceHistoryEntry {
                    lot_id: LOT_ID,
                    auction_id: AUCTION_ID,
                    world_id: "main".to_owned(),
                    auction_sequence: 5,
                    ordering_lot_number: 2,
                    lot_number: 1,
                    equipment_name: "Legendary Staff".to_owned(),
                    equipment_category: "Staff".to_owned(),
                    equipment_level: Some(450),
                    buyer_name: "buyer".to_owned(),
                    seller_name: "seller".to_owned(),
                    price: 2_000,
                    closed_at: at,
                };
                let older = PublicPriceHistoryEntry {
                    lot_id: Uuid::from_u128(0x4000_8000_0000_0000_0003),
                    ordering_lot_number: 1,
                    lot_number: 2,
                    equipment_level: None,
                    price: 2_001,
                    ..row.clone()
                };
                if request.world_id.as_deref() == Some("isekai") {
                    let row = PublicPriceHistoryEntry { world_id: "isekai".to_owned(), ..older };
                    return Ok(PublicPriceHistoryPage {
                        entries: vec![row],
                        total: Some(1),
                        stats: Some(PublicPriceHistoryStats {
                            count: 1,
                            minimum: 2_001,
                            maximum: 2_001,
                            median: 2_001,
                            median_exact: Some("2001".to_owned()),
                        }),
                        limit: request.limit,
                        offset: request.offset,
                        next_cursor: None,
                        has_more: false,
                    });
                }
                Ok(PublicPriceHistoryPage {
                    entries: vec![row, older],
                    total: request.cursor.is_none().then_some(2),
                    stats: request.cursor.is_none().then(|| PublicPriceHistoryStats {
                        count: 2,
                        minimum: 2_000,
                        maximum: 2_001,
                        median: 2_000,
                        median_exact: Some("2000.5".to_owned()),
                    }),
                    limit: request.limit,
                    offset: request.offset,
                    next_cursor: None,
                    has_more: false,
                })
            })
        }
    }

    #[tokio::test]
    async fn history_read_contract_fixtures() {
        for (name, query, final_phase) in [
            ("bid-log", "", false),
            ("bid-log-defeated-proxy", "include=defeated_proxy", true),
            ("bid-log-cursor", "cursor=start&include=defeated_proxy", true),
        ] {
            let (status, body) = request_bid_log(query, final_phase).await;
            assert_eq!(status, StatusCode::OK, "{name}: {body}");
            assert_wire(&format!("public/{name}"), body);
        }

        let state = PublicApiState::new(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(FixedPriceHistory)),
            None,
            None,
            None,
            None,
            PublicHttpSecurity::default(),
        );
        for (name, uri) in [
            ("price-history", "/api/v1/price-history"),
            ("price-history-cursor", "/api/v1/price-history?cursor=start"),
            ("price-history-unleveled", "/api/v1/price-history?world=isekai"),
        ] {
            let response = list_price_history(
                axum::extract::State(state.clone()),
                uri.parse().expect("成交历史 URI"),
            )
            .await
            .expect("成交历史");
            assert_http(&format!("public/{name}"), StatusCode::OK, response).await;
        }
    }
}
