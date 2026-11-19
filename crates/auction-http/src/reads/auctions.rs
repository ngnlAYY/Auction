//! 公开拍卖列表与当前场次的处理器。

use auction_application::auction::{PublicAuctionListRequest, PublicAuctionSummary};
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_contracts::{ApiErrorResponse, ApiResponse, CurrentAuctionResponse};
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use super::mapping::{
    map_auction_page, map_auction_summary, map_auction_summary_v2, map_current_auction,
};
use crate::diagnostics;
use crate::errors::{api_error, query_rejection};
use crate::mapping::decode_public_cursor;
use crate::middleware::response_cache::LOW_FREQUENCY_CACHE_CONTROL;
use crate::query::{page_offset, parse_positive_i64, parse_public_page_size, parse_query};
use crate::state::PublicApiState;

/// 保留 v1 字段闭集，已打开的旧页面无需接受新的摘要字段。
pub(crate) async fn list_auctions(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    auction_list(state, uri, false, map_auction_summary).await
}

/// v2 在同一分页读取中附带论坛地址，避免按行发起额外 HTTP 或数据库请求。
pub(crate) async fn list_auctions_v2(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    auction_list(state, uri, true, map_auction_summary_v2).await
}

/// 两个版本共享筛选、分页、游标和错误处理，只有投影选择与条目映射不同。
async fn auction_list<T>(
    state: PublicApiState,
    uri: Uri,
    include_forum_thread_url: bool,
    map_summary: impl FnMut(PublicAuctionSummary) -> T,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)>
where
    T: serde::Serialize,
{
    let mut query = parse_query(&uri, &["world", "status", "page", "page_size", "cursor"])
        .map_err(|()| query_rejection())?;
    let world_id = query.remove("world").filter(|value| !value.is_empty());
    if world_id.as_deref().is_some_and(|world| world.len() > 64 || world != world.trim()) {
        return Err(query_rejection());
    }

    let status = query
        .remove("status")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "settled".to_owned());
    if !matches!(status.as_str(), "live" | "final" | "settled") {
        return Err(query_rejection());
    }

    let page = parse_positive_i64(query.get("page"), 1, None).map_err(|()| query_rejection())?;
    let page_size =
        parse_public_page_size(query.get("page_size"), 20).map_err(|()| query_rejection())?;
    let raw_cursor = query.remove("cursor");
    let cursor = raw_cursor
        .as_deref()
        .map(decode_public_cursor)
        .transpose()
        .map_err(|()| query_rejection())?;
    if cursor.is_some() && page != 1 {
        return Err(query_rejection());
    }
    let offset = if cursor.is_some() {
        0
    } else {
        page_offset(page, page_size, Some(10_000)).ok_or_else(query_rejection)?
    };

    let settled = status == "settled";
    let Some(reader) = state.auctions() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖目录暂不可用",
        ));
    };
    let result = reader
        .list_public_auctions(PublicAuctionListRequest {
            world_id,
            status,
            limit: page_size,
            offset,
            cursor,
            include_forum_thread_url,
        })
        .await
        .map_err(|diagnostic_cause| {
            diagnostics::warn(
                "public_catalog",
                "read_auctions",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "拍卖目录暂不可用")
        })?;
    if result.limit != page_size || result.offset != offset {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "拍卖目录暂不可用",
        ));
    }

    let response = map_auction_page(result, page, raw_cursor, map_summary);
    let mut response = Json(ApiResponse::ok("拍卖列表已加载", response)).into_response();
    if settled {
        // 已结算列表只会在新场次结算时追加；低频档避免每次翻页回源。
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(LOW_FREQUENCY_CACHE_CONTROL));
    }
    Ok(response)
}

/// 条目缺省保持 v1 字段闭集：已打开的旧页面按闭集解码，收到未知键会整条拒绝。冻结策略的
/// 代理默认模式只在 `include=proxy_default_mode` 显式请求时追加。
pub(crate) async fn current_auction(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<Json<ApiResponse<CurrentAuctionResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let mut query = parse_query(&uri, &["world", "include"]).map_err(|()| query_rejection())?;
    let world_id = query
        .remove("world")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "main".to_owned());
    if world_id.len() > 64 || world_id != world_id.trim() {
        return Err(query_rejection());
    }
    // 只接受登记的附加字段；未知值失败关闭，不静默退回原字段集合。
    let include_proxy_default_mode = match query.remove("include").as_deref() {
        None => false,
        Some("proxy_default_mode") => true,
        Some(_) => {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "附加字段无效"));
        }
    };
    let Some(reader) = state.auctions() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "当前拍卖暂不可用",
        ));
    };
    let auction = reader.current_auction(world_id).await.map_err(|diagnostic_cause| {
        diagnostics::warn(
            "public_catalog",
            "read_current_auction",
            DiagnosticError::diagnostic_kind(&diagnostic_cause),
            DiagnosticContext::default(),
        );

        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "当前拍卖暂不可用")
    })?;
    Ok(Json(ApiResponse::ok(
        if auction.is_some() { "当前拍卖已加载" } else { "当前没有进行中的拍卖" },
        CurrentAuctionResponse {
            auction: auction
                .map(|auction| map_current_auction(auction, include_proxy_default_mode)),
        },
    )))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::auction::{
        AuctionReader, CurrentAuctionSnapshot, PublicAuctionCountdown, PublicAuctionListRequest,
        PublicAuctionPage, PublicAuctionSummary, PublicRealtimeAuctionStatus,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::forum::PublicForumLink;
    use auction_domain::ProxyMode;
    use auction_time::{Duration, Timestamp};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{current_auction, list_auctions, list_auctions_v2};
    use crate::middleware::security::PublicHttpSecurity;
    use crate::server::contract_fixtures::{assert_http, assert_wire};
    use crate::state::PublicApiState;

    const AUCTION_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b);

    /// 提供可固定的当前场次与目录投影；倒计时与实时预检不参与此处的读取。
    struct FixedCurrent(Option<CurrentAuctionSnapshot>);

    impl AuctionReader for FixedCurrent {
        fn list_public_auctions(
            &self,
            request: PublicAuctionListRequest,
        ) -> ApplicationFuture<'_, PublicAuctionPage> {
            Box::pin(async move {
                let opened = Timestamp::from_timestamp(1_763_200_000, 0).expect("开拍时刻");
                let settled = request.status == "settled";
                Ok(PublicAuctionPage {
                    entries: vec![PublicAuctionSummary {
                        id: AUCTION_ID,
                        world_id: "main".to_owned(),
                        sequence: 5,
                        status: request.status,
                        scheduled_open_at: opened,
                        opened_at: Some(opened),
                        final_phase_at: settled.then(|| opened + Duration::days(1)),
                        effective_end_at: Some(opened + Duration::days(2)),
                        settled_at: settled.then(|| opened + Duration::days(2)),
                        lot_count: 2,
                        sold_count: i64::from(settled),
                        unsold_count: i64::from(settled),
                        sold_amount: if settled { 2_000 } else { 0 },
                        state_version: if settled { 8 } else { 3 },
                        forum_link: (settled && request.include_forum_thread_url).then(|| {
                            PublicForumLink::new(
                                "https://forums.e-hentai.org/index.php?showtopic=123".to_owned(),
                                1,
                                Some(opened),
                            )
                            .expect("公开论坛地址")
                        }),
                    }],
                    total: request.cursor.is_none().then_some(1),
                    limit: request.limit,
                    offset: request.offset,
                    next_cursor: None,
                    has_more: false,
                })
            })
        }

        fn current_auction(
            &self,
            _world_id: String,
        ) -> ApplicationFuture<'_, Option<CurrentAuctionSnapshot>> {
            let auction = self.0.clone();
            Box::pin(async move { Ok(auction) })
        }

        fn public_auction_countdown(
            &self,
            _auction_id: Uuid,
        ) -> ApplicationFuture<'_, Option<PublicAuctionCountdown>> {
            Box::pin(async { Ok(None) })
        }

        fn public_realtime_auction_status(
            &self,
            _auction_id: Uuid,
        ) -> ApplicationFuture<'_, Option<PublicRealtimeAuctionStatus>> {
            Box::pin(async { Ok(None) })
        }
    }

    fn live(proxy_default_mode: ProxyMode) -> CurrentAuctionSnapshot {
        let opens_at = Timestamp::from_timestamp(1_763_200_000, 0).expect("fixture time");
        CurrentAuctionSnapshot {
            id: AUCTION_ID,
            world_id: "main".to_owned(),
            sequence: 5,
            status: "live".to_owned(),
            opens_at,
            scheduled_ends_at: Some(opens_at + Duration::days(2)),
            presentation_mode: "live".to_owned(),
            bidding_open: true,
            preview_revision: 0,
            proxy_default_mode,
        }
    }

    fn state(auction: Option<CurrentAuctionSnapshot>) -> PublicApiState {
        PublicApiState::new(
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(FixedCurrent(auction))),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            PublicHttpSecurity::default(),
        )
    }

    async fn request(auction: Option<CurrentAuctionSnapshot>, query: &str) -> (StatusCode, Value) {
        let app = Router::new()
            .route("/api/v1/auctions/current", get(current_auction))
            .with_state(state(auction));
        let separator = if query.is_empty() { "" } else { "?" };
        let uri = format!("/api/v1/auctions/current{separator}{query}");
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        (status, serde_json::from_slice(&body).expect("json body"))
    }

    /// 回归：不带 `include` 时条目仍是 v1 原字段集合，已打开的旧页面的闭集解码照常接受。
    #[tokio::test]
    async fn current_auction_keeps_the_original_field_set_by_default() {
        for query in ["", "world=main"] {
            let (status, body) = request(Some(live(ProxyMode::Slow)), query).await;
            assert_eq!(status, StatusCode::OK, "{query}");
            let mut keys = body["data"]["auction"]
                .as_object()
                .expect("auction object")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "bidding_open",
                    "id",
                    "number",
                    "opens_at",
                    "presentation_mode",
                    "preview_revision",
                    "scheduled_ends_at",
                    "status",
                    "world",
                ],
                "{query}"
            );
        }
    }

    /// `include=proxy_default_mode` 只追加冻结策略的代理默认模式，其余字段与缺省响应逐字段相同。
    #[tokio::test]
    async fn include_adds_only_the_frozen_proxy_default_mode() {
        for (mode, name) in [(ProxyMode::Fast, "fast"), (ProxyMode::Slow, "slow")] {
            let (status, original) = request(Some(live(mode)), "world=main").await;
            assert_eq!(status, StatusCode::OK);
            let (status, included) =
                request(Some(live(mode)), "world=main&include=proxy_default_mode").await;
            assert_eq!(status, StatusCode::OK);
            let mut expected = original;
            expected["data"]["auction"]["proxy_default_mode"] = json!(name);
            assert_eq!(included, expected, "{name}");
        }
        // 没有当前场次时仍是 `auction: null`，附加字段不改变空结果。
        let (status, empty) = request(None, "include=proxy_default_mode").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(empty["data"]["auction"], Value::Null);
    }

    #[tokio::test]
    async fn unknown_or_repeated_include_fails_closed() {
        for (query, message) in [
            ("include=", "附加字段无效"),
            ("include=PROXY_DEFAULT_MODE", "附加字段无效"),
            ("include=final_restriction", "附加字段无效"),
            ("include=proxy_default_mode,final_restriction", "附加字段无效"),
            ("include=proxy_default_mode&include=proxy_default_mode", "查询参数无效"),
            ("fields=proxy_default_mode", "查询参数无效"),
        ] {
            let (status, body) = request(Some(live(ProxyMode::Fast)), query).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], message, "{query}");
        }
    }

    #[tokio::test]
    async fn auction_read_contract_fixtures() {
        let mut preview = live(ProxyMode::Fast);
        preview.status = "preview".to_owned();
        preview.presentation_mode = "opening".to_owned();
        preview.bidding_open = false;
        preview.scheduled_ends_at = None;
        preview.preview_revision = 2;
        for (name, auction, query) in [
            ("auction-current", Some(live(ProxyMode::Slow)), ""),
            (
                "auction-current-proxy-mode",
                Some(live(ProxyMode::Slow)),
                "include=proxy_default_mode",
            ),
            ("auction-current-opening", Some(preview), "include=proxy_default_mode"),
            ("auction-current-empty", None, "include=proxy_default_mode"),
        ] {
            let (status, body) = request(auction, query).await;
            assert_eq!(status, StatusCode::OK);
            assert_wire(&format!("public/{name}"), body);
        }

        for (name, uri, v2) in [
            ("auctions-v1", "/api/v1/auctions", false),
            ("auctions-v1-live", "/api/v1/auctions?status=live", false),
            ("auctions-v2", "/api/v2/auctions", true),
            ("auctions-v2-live", "/api/v2/auctions?status=live", true),
            ("auctions-v1-cursor", "/api/v1/auctions?cursor=start", false),
            ("auctions-v2-cursor", "/api/v2/auctions?cursor=start", true),
        ] {
            let query = uri.parse().expect("目录 URI");
            let response = if v2 {
                list_auctions_v2(axum::extract::State(state(None)), query).await
            } else {
                list_auctions(axum::extract::State(state(None)), query).await
            };
            assert_http(&format!("public/{name}"), StatusCode::OK, response.expect("场次目录"))
                .await;
        }
    }
}
