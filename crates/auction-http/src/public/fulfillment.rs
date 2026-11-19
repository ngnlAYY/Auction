//! Public 履约进度 HTTP 适配器：解析查询闭集、调用只读端口，把应用层的闭集状态与附带字段映射为
//! 稳定 DTO。判定规则与失败关闭都在应用层，这里不重建任何状态机。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::public::{
    DEFAULT_PUBLIC_FULFILLMENT_PAGE_SIZE, MAX_PUBLIC_FULFILLMENT_OFFSET,
    MAX_PUBLIC_FULFILLMENT_PAGE_SIZE, PublicFulfillmentLot, PublicFulfillmentPage,
    PublicFulfillmentRequest, PublicFulfillmentView,
};
use auction_contracts::{
    ApiErrorResponse, ApiResponse, PublicFulfillmentAuctionResponse, PublicFulfillmentLotResponse,
    PublicFulfillmentPageResponse,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};

use crate::diagnostics;
use crate::errors::{api_error, not_found_error, query_rejection};
use crate::mapping::public_timestamp;
use crate::query::{page_offset, parse_positive_i64, parse_public_uuid, parse_query};
use crate::state::PublicApiState;

/// 已结算场次逐件的履约进度。查询参数闭集为 `world`、`lot`、`page`、`page_size`；`page_size`
/// 默认 50、上限 100。场次不存在、未结算或不可见为 404。能解释但不足以判定的事实是合法的
/// `unknown` 状态；端口未注入，或事实含闭集外原值、重复或自相矛盾时整页 503。
pub(crate) async fn list(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<
    (HeaderMap, Json<ApiResponse<PublicFulfillmentPageResponse>>),
    (StatusCode, Json<ApiErrorResponse>),
> {
    let auction_id = parse_public_uuid(&auction_id)
        .map_err(|()| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let mut query = parse_query(&uri, &["world", "lot", "page", "page_size"])
        .map_err(|()| query_rejection())?;
    let world_id = match query.remove("world") {
        None => None,
        Some(world) if world.is_empty() || world.len() > 64 || world != world.trim() => {
            return Err(query_rejection());
        }
        Some(world) => Some(world),
    };
    // lot 筛选与竞价日志同口径：规范 UUID 精确匹配，否则按不超过 32 位字母数字的 lot 代码匹配。
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
    let page_size = parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PUBLIC_FULFILLMENT_PAGE_SIZE,
        Some(MAX_PUBLIC_FULFILLMENT_PAGE_SIZE),
    )
    .map_err(|()| query_rejection())?;
    let offset = page_offset(page, page_size, Some(MAX_PUBLIC_FULFILLMENT_OFFSET))
        .ok_or_else(query_rejection)?;
    let request = PublicFulfillmentRequest {
        auction_id,
        world_id,
        lot_id,
        lot_code,
        limit: page_size,
        offset,
    };
    if request.validate().is_err() {
        return Err(query_rejection());
    }
    let world_id = request.world_id.clone();

    let Some(reader) = state.fulfillment() else {
        return Err(unavailable());
    };
    let result = reader.list_public_fulfillment(request).await.map_err(map_reader_error)?;
    let Some(result) = result else {
        return Err(not_found_error());
    };
    // 返回页必须对应本次请求的场次、世界、分页与 lot；任一不符说明适配器出错，失败关闭。
    if result.auction.auction_id != auction_id
        || world_id.is_some_and(|world| world != result.auction.world_id)
        || result.limit != page_size
        || result.offset != offset
        || lot_id.is_some_and(|lot_id| result.entries.iter().any(|lot| lot.lot_id != lot_id))
    {
        return Err(unavailable());
    }
    let Ok(views) = result.validate().and_then(|()| result.views()) else {
        return Err(unavailable());
    };

    let mut headers = HeaderMap::new();
    // 履约状态随寄出、领取、付款与退回持续变化，分钟级缓存会让玩家看到过时的付款或退件状态：
    // 与补偿池汇总一致使用 `no-cache`，浏览器与 CDN 可以保存，但每次使用前都向源站再验证，
    // 未变化时由内容 ETag 回 304，开销仍低。
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((headers, Json(ApiResponse::ok("履约进度已加载", map_page(result, views, page)))))
}

fn map_page(
    page: PublicFulfillmentPage,
    views: Vec<PublicFulfillmentView>,
    page_number: i64,
) -> PublicFulfillmentPageResponse {
    PublicFulfillmentPageResponse {
        auction: PublicFulfillmentAuctionResponse {
            id: page.auction.auction_id.to_string(),
            world: page.auction.world_id,
            number: page.auction.sequence,
        },
        lots: page.entries.into_iter().zip(views).map(|(lot, view)| map_lot(lot, view)).collect(),
        page: page_number,
        page_size: page.limit,
        total: page.total,
    }
}

fn map_lot(lot: PublicFulfillmentLot, view: PublicFulfillmentView) -> PublicFulfillmentLotResponse {
    let (buyer_name, final_price) =
        view.sale.map(|sale| (sale.buyer_name, sale.final_price)).unzip();
    let (cod_return_count, cod_return_limit) =
        view.cod_returns.map(|returns| (returns.count, returns.limit)).unzip();
    PublicFulfillmentLotResponse {
        lot_id: lot.lot_id.to_string(),
        lot_number: lot.lot_number,
        equipment_name: lot.equipment_name,
        equipment_category: lot.equipment_category,
        seller_name: lot.seller_name,
        buyer_name,
        final_price,
        status: view.status.as_str().to_owned(),
        // 期限对外只到秒：玩家按秒级时刻判断，亚秒部分只会让同一期限显示成不同字符串。
        payment_deadline_at: view
            .payment_deadline_at
            .map(|deadline| public_timestamp(deadline.trunc_subsecs(0))),
        cod_return_count,
        cod_return_limit,
    }
}

fn unavailable() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "履约进度暂不可用")
}

/// 读端口只有数据库实现，任何读取失败都按依赖不可用折叠；折叠前先投影安全分类。
fn map_reader_error(error: ApplicationError) -> (StatusCode, Json<ApiErrorResponse>) {
    diagnostics::warn(
        "public_fulfillment",
        "read_fulfillment",
        DiagnosticError::diagnostic_kind(&error),
        DiagnosticContext::default(),
    );
    unavailable()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use auction_application::core::{ApplicationError, ApplicationFuture};
    use auction_application::public::{
        PublicFulfillmentAuction, PublicFulfillmentLot, PublicFulfillmentObligation,
        PublicFulfillmentPage, PublicFulfillmentReader, PublicFulfillmentRecovery,
        PublicFulfillmentRequest,
    };
    use auction_time::{Duration, Timestamp};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use axum::routing::get;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::list;
    use crate::server::contract_fixtures::assert_wire;
    use crate::state::PublicApiState;

    const AUCTION_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b);
    const PATH: &str = "/api/v1/auctions/0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b/fulfillment";

    fn obligation(
        kind: &str,
        business_state: &str,
        execution_state: &str,
        delivery_status: Option<&str>,
    ) -> PublicFulfillmentObligation {
        PublicFulfillmentObligation {
            kind: kind.to_owned(),
            business_state: business_state.to_owned(),
            execution_state: Some(execution_state.to_owned()),
            delivery_status: delivery_status.map(str::to_owned),
        }
    }

    fn read_at() -> Timestamp {
        Timestamp::from_timestamp(1_763_200_000, 0).expect("固定读取时刻")
    }

    fn recovery(stage: &str, cod_return_count: i32) -> PublicFulfillmentRecovery {
        PublicFulfillmentRecovery {
            stage: stage.to_owned(),
            resolution: None,
            // 带亚秒部分：响应只保留到秒。
            payment_deadline_at: read_at() + Duration::days(1) + Duration::milliseconds(750),
            cod_return_count,
            cod_return_limit: 3,
            blocking_code: Some("recovery_delivery_failed".to_owned()),
            manual_reconcile: false,
            relisted: false,
        }
    }

    fn lot(
        id: u128,
        lot_status: &str,
        recovery: Option<PublicFulfillmentRecovery>,
        obligations: Vec<PublicFulfillmentObligation>,
    ) -> PublicFulfillmentLot {
        let sold = lot_status == "sold";
        PublicFulfillmentLot {
            lot_id: Uuid::from_u128(0x4000_8000_0000_0000_0000 + id),
            lot_number: i32::try_from(id).expect("序号"),
            equipment_name: format!("Equipment {id}"),
            equipment_category: "Cloth".to_owned(),
            seller_name: "seller".to_owned(),
            lot_status: lot_status.to_owned(),
            buyer_name: sold.then(|| "buyer".to_owned()),
            final_price: Some(if sold { 1_250_000 } else { 0 }),
            equipment_lost: false,
            recovery,
            obligations,
        }
    }

    /// 三件 lot：流拍待寄回、COD 已寄出等待付款（付款提醒失败的阻断码不公开）、COD 退回后重寄。
    fn entries() -> Vec<PublicFulfillmentLot> {
        vec![
            lot(1, "unsold", None, vec![obligation("unsold_return", "ready", "pending", None)]),
            lot(
                2,
                "sold",
                Some(recovery("awaiting_payment", 0)),
                vec![
                    obligation("winner_equipment_cod", "ready", "enqueued", Some("succeeded")),
                    obligation("seller_payout", "blocked", "pending", None),
                    obligation("buyer_refund", "contingent", "pending", None),
                ],
            ),
            lot(
                3,
                "sold",
                Some(recovery("awaiting_payment", 1)),
                vec![
                    obligation("winner_equipment_cod", "ready", "enqueued", Some("pending")),
                    obligation("seller_payout", "blocked", "pending", None),
                    obligation("buyer_refund", "contingent", "pending", None),
                ],
            ),
        ]
    }

    /// 记录收到的请求并返回固定结果的假读端口。
    struct FakeReader {
        result: Result<Option<Vec<PublicFulfillmentLot>>, ()>,
        requests: Mutex<Vec<PublicFulfillmentRequest>>,
    }

    impl FakeReader {
        fn new(result: Result<Option<Vec<PublicFulfillmentLot>>, ()>) -> Arc<Self> {
            Arc::new(Self { result, requests: Mutex::new(Vec::new()) })
        }

        fn requests(&self) -> Vec<PublicFulfillmentRequest> {
            self.requests.lock().expect("请求记录").clone()
        }
    }

    impl PublicFulfillmentReader for FakeReader {
        fn list_public_fulfillment(
            &self,
            request: PublicFulfillmentRequest,
        ) -> ApplicationFuture<'_, Option<PublicFulfillmentPage>> {
            self.requests.lock().expect("请求记录").push(request.clone());
            let result = self.result.clone();
            Box::pin(async move {
                let entries =
                    result.map_err(|()| ApplicationError::CapabilityUnavailable("fake"))?;
                Ok(entries.map(|entries| PublicFulfillmentPage {
                    auction: PublicFulfillmentAuction {
                        auction_id: request.auction_id,
                        world_id: "main".to_owned(),
                        sequence: 12,
                    },
                    read_at: read_at(),
                    total: i64::try_from(entries.len()).expect("数量"),
                    entries,
                    limit: request.limit,
                    offset: request.offset,
                }))
            })
        }
    }

    async fn call(
        reader: Option<Arc<FakeReader>>,
        query: &str,
    ) -> (StatusCode, Option<String>, Value) {
        let state = PublicApiState::default();
        let state = match reader {
            Some(reader) => state.with_fulfillment(reader),
            None => state,
        };
        let app = Router::new()
            .route("/api/v1/auctions/{auction_id}/fulfillment", get(list))
            .with_state(state);
        let separator = if query.is_empty() { "" } else { "?" };
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("{PATH}{separator}{query}"))
                    .body(Body::empty())
                    .expect("请求"),
            )
            .await
            .expect("响应");
        let status = response.status();
        let cache = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("正文");
        (status, cache, serde_json::from_slice(&body).expect("JSON 正文"))
    }

    fn keys(value: &Value) -> Vec<&str> {
        let mut keys =
            value.as_object().expect("对象").keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    #[tokio::test]
    async fn page_exposes_only_the_closed_field_set() {
        let (status, cache, body) = call(Some(FakeReader::new(Ok(Some(entries())))), "").await;
        assert_eq!(status, StatusCode::OK);
        // 状态会随履约推进变化：每次使用前都必须再验证，不得是分钟级的可直接复用缓存档。
        assert_eq!(cache.as_deref(), Some("no-cache"));
        let data = &body["data"];
        assert_eq!(keys(data), ["auction", "lots", "page", "page_size", "total"]);
        assert_eq!(
            data["auction"],
            json!({ "id": AUCTION_ID.to_string(), "world": "main", "number": 12 })
        );
        assert_eq!((data["page"].clone(), data["page_size"].clone()), (json!(1), json!(50)));
        let lots = data["lots"].as_array().expect("lots");
        // 流拍：没有买家、成交价、付款期限与退回次数。
        assert_eq!(
            keys(&lots[0]),
            [
                "equipment_category",
                "equipment_name",
                "lot_id",
                "lot_number",
                "seller_name",
                "status"
            ]
        );
        assert_eq!(lots[0]["status"], "unsold_return_pending");
        // 等待付款：成交事实与秒精度的付款期限；付款提醒失败的阻断码不出现。
        assert_eq!(lots[1]["status"], "awaiting_payment");
        assert_eq!(lots[1]["buyer_name"], "buyer");
        assert_eq!(lots[1]["final_price"], 1_250_000);
        assert_eq!(lots[1]["payment_deadline_at"], "2025-11-16T09:46:40Z");
        assert!(lots[1].get("cod_return_count").is_none());
        // 退回后待重寄：退回次数与上限成对出现，尚未寄出时不公开期限。
        assert_eq!(lots[2]["status"], "cod_resend_pending");
        assert_eq!(
            (lots[2]["cod_return_count"].clone(), lots[2]["cod_return_limit"].clone()),
            (json!(1), json!(3))
        );
        assert!(lots[2].get("payment_deadline_at").is_none());
        let serialized = body.to_string();
        for forbidden in ["blocking", "recovery_delivery_failed", "obligation", "delivery", "mode"]
        {
            assert!(!serialized.contains(forbidden), "{forbidden}");
        }
    }

    #[tokio::test]
    async fn query_is_parsed_into_the_port_request() {
        let reader = FakeReader::new(Ok(Some(Vec::new())));
        let lot_id = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";
        for query in [
            "page=3&page_size=10&world=main&lot=Clo12".to_owned(),
            format!("lot={lot_id}&page_size=100"),
        ] {
            let (status, _, _) = call(Some(Arc::clone(&reader)), &query).await;
            assert_eq!(status, StatusCode::OK, "{query}");
        }
        let requests = reader.requests();
        assert_eq!(
            (requests[0].world_id.as_deref(), requests[0].lot_code.as_deref(), requests[0].lot_id),
            (Some("main"), Some("Clo12"), None)
        );
        assert_eq!((requests[0].limit, requests[0].offset), (10, 20));
        assert_eq!(requests[1].lot_id, Some(Uuid::parse_str(lot_id).expect("lot UUID")));
        assert_eq!((requests[1].limit, requests[1].offset), (100, 0));
    }

    #[tokio::test]
    async fn invalid_queries_are_rejected_before_reading() {
        let reader = FakeReader::new(Ok(Some(entries())));
        for query in [
            "page_size=101",
            "page_size=0",
            "page=0",
            "page=10002",
            "unknown=1",
            "page=1&page=2",
            "lot=",
            "lot=Clo-12",
            "world=",
            "world=%20main",
            "cursor=abc",
        ] {
            let (status, cache, body) = call(Some(Arc::clone(&reader)), query).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert!(cache.is_none(), "{query}");
        }
        assert!(reader.requests().is_empty(), "非法查询不得读取端口");
    }

    #[tokio::test]
    async fn invalid_auction_id_is_rejected() {
        let app = Router::new()
            .route("/api/v1/auctions/{auction_id}/fulfillment", get(list))
            .with_state(PublicApiState::default());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auctions/0190F2B4-1C2D-4E5F-8A6B-7C8D9E0F1A2B/fulfillment")
                    .body(Body::empty())
                    .expect("请求"),
            )
            .await
            .expect("响应");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn missing_auction_port_and_failures_have_stable_errors() {
        let (status, _, body) = call(Some(FakeReader::new(Ok(None))), "").await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
        for reader in [None, Some(FakeReader::new(Err(())))] {
            let (status, cache, body) = call(reader, "").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["code"], "storage_unavailable");
            assert!(cache.is_none());
        }
    }

    #[tokio::test]
    async fn unknown_facts_fail_the_whole_page_closed() {
        let mut facts = entries();
        facts[1].obligations[0].execution_state = Some("archived".to_owned());
        let (status, _, body) = call(Some(FakeReader::new(Ok(Some(facts)))), "").await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some("storage_unavailable"))
        );
    }

    #[tokio::test]
    async fn page_for_another_lot_fails_closed() {
        let reader = FakeReader::new(Ok(Some(entries())));
        let (status, _, _) = call(Some(reader), "lot=0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "返回页含筛选之外的 lot");
    }

    #[tokio::test]
    async fn fulfillment_contract_fixtures() {
        for (name, entries) in [
            ("public/fulfillment", vec![entries().remove(1)]),
            ("public/fulfillment-recovery", entries()),
        ] {
            let (status, _, body) = call(Some(FakeReader::new(Ok(Some(entries)))), "").await;
            assert_eq!(status, StatusCode::OK, "{name}");
            assert_wire(name, body);
        }
    }
}
