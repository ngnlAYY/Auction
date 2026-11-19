//! 公开 lot 列表与详情的处理器。

use auction_application::auction::{
    MAX_PUBLIC_LOT_OFFSET, PublicLotCategorySelection, PublicLotListRequest,
};
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError, SafeErrorKind};
use auction_application::public::PublicEquipmentLinkTarget;
use auction_contracts::{ApiErrorResponse, ApiResponse};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use super::mapping::{
    LotListFields, lot_detail_equipment_target, lot_page_equipment_targets, map_lot_detail,
    map_lot_page, valid_lot_detail,
};
use crate::diagnostics;
use crate::errors::{api_error, not_found_error, query_rejection};
use crate::middleware::response_cache::SETTLED_CACHE_CONTROL;
use crate::query::{
    page_offset, parse_positive_i64, parse_public_page_size, parse_public_uuid, parse_query,
};
use crate::state::PublicApiState;

/// 公开 lot 列表。preview 与 live 使用互斥的数组形状，保持已发布的 JSON 契约。
pub(crate) async fn list_lots(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = parse_public_uuid(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let query = parse_query(
        &uri,
        &[
            "category",
            "equipment_group",
            "max_level",
            "include_unleveled",
            "page",
            "page_size",
            "fields",
        ],
    )
    .map_err(|()| query_rejection())?;
    let page = parse_positive_i64(query.get("page"), 1, None).map_err(|()| query_rejection())?;
    let page_size =
        parse_public_page_size(query.get("page_size"), 20).map_err(|()| query_rejection())?;
    let offset =
        page_offset(page, page_size, Some(MAX_PUBLIC_LOT_OFFSET)).ok_or_else(query_rejection)?;
    let equipment_group = query
        .get("equipment_group")
        .map(|value| {
            auction_application::forum::EquipmentGroup::parse(value)
                .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "装备分组无效"))
        })
        .transpose()?;
    let max_level = query
        .get("max_level")
        .map(|value| {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_query",
                    "装备等级上限无效",
                ));
            }
            value.parse::<i64>().map_err(|_| {
                api_error(StatusCode::BAD_REQUEST, "invalid_query", "装备等级上限无效")
            })
        })
        .transpose()?;
    let include_unleveled = match query.get("include_unleveled").map(String::as_str) {
        None | Some("true") => true,
        Some("false") => false,
        _ => return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "无等级筛选无效")),
    };
    // 投影只是可选的精简形状：缺省保持 v1 原字段集合，未知值失败关闭而不是静默回退完整投影。
    let fields = match query.get("fields").map(String::as_str) {
        None => LotListFields::Full,
        Some("summary") => LotListFields::Summary,
        _ => return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "字段投影无效")),
    };
    // 省略参数传入零项得到“不限分类”，显式空串仍是单个空项并失败关闭；
    // 按借用的分段逐项校验，不先收集 String，非法项或第 8 项由构造器立即拒绝。
    let category_selection = PublicLotCategorySelection::from_public_keys(
        query.get("category").into_iter().flat_map(|value| value.split(',')),
    )
    .map_err(|_| query_rejection())?;
    let request = PublicLotListRequest {
        category_selection,
        equipment_group,
        max_level,
        include_unleveled,
        limit: page_size,
        offset,
    };
    if request.validate().is_err() {
        return Err(query_rejection());
    }

    let Some(reader) = state.lots() else {
        return Err(lot_dependency_unavailable());
    };
    let page_result =
        reader.list_public_lots(auction_id, request.clone()).await.map_err(|diagnostic_cause| {
            diagnostics::warn(
                "public_catalog",
                "read_lot_page",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            lot_dependency_unavailable()
        })?;
    let Some(mut page_result) = page_result else {
        return Err(not_found_error());
    };
    if page_result.validate(auction_id, &request).is_err()
        || page_result.validate_filters(&request).is_err()
        || page_result.limit != page_size
        || page_result.offset != offset
    {
        return Err(lot_dependency_unavailable());
    }
    let targets = lot_page_equipment_targets(&mut page_result).map_err(|diagnostic_cause| {
        diagnostics::error(
            "public_catalog",
            "project_lot_page",
            diagnostics::invalid_state(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        lot_dependency_unavailable()
    })?;
    let game_equipment_urls = resolve_equipment_urls(&state, &targets).await?;
    let response = map_lot_page(page_result, page, game_equipment_urls, fields)
        .ok_or_else(lot_dependency_unavailable)?;
    // 已结算批次的状态与金额不再变化（领先者名随改名滞后更新）；明细接口同样由各自状态
    // 决定，未结算仍走 2 秒目录缓存。
    let settled = response.auction_status == "settled";
    let mut response = Json(ApiResponse::ok("拍卖批次已加载", response)).into_response();
    if settled {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(SETTLED_CACHE_CONTROL));
    }
    Ok(response)
}

/// 公开 lot 详情只返回仍属于当前公开 projection 的对象。
pub(crate) async fn lot_detail(
    State(state): State<PublicApiState>,
    Path(lot_id): Path<String>,
    uri: Uri,
) -> Result<Response, (StatusCode, Json<ApiErrorResponse>)> {
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid_query", "lot 详情不接受查询参数"));
    }
    let lot_id = parse_public_uuid(&lot_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效"))?;
    let Some(reader) = state.lots() else {
        return Err(lot_dependency_unavailable());
    };
    let detail = reader.public_lot(lot_id).await.map_err(|diagnostic_cause| {
        diagnostics::warn(
            "public_catalog",
            "read_lot_detail",
            DiagnosticError::diagnostic_kind(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        lot_dependency_unavailable()
    })?;
    let Some(mut detail) = detail else {
        return Err(not_found_error());
    };
    if !valid_lot_detail(&detail, lot_id) {
        return Err(lot_dependency_unavailable());
    }
    let target = lot_detail_equipment_target(&mut detail, lot_id).map_err(|diagnostic_cause| {
        diagnostics::error(
            "public_catalog",
            "project_lot_detail",
            diagnostics::invalid_state(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        lot_dependency_unavailable()
    })?;
    let mut game_equipment_urls = resolve_equipment_urls(&state, &[target]).await?;
    let game_equipment_url = game_equipment_urls.pop().filter(|url| !url.is_empty());
    let response = map_lot_detail(detail, game_equipment_url);
    let settled = response.auction_status == "settled";
    let mut response = Json(ApiResponse::ok("拍卖批次已加载", response)).into_response();
    if settled {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(SETTLED_CACHE_CONTROL));
    }
    Ok(response)
}

async fn resolve_equipment_urls(
    state: &PublicApiState,
    targets: &[PublicEquipmentLinkTarget],
) -> Result<Vec<String>, (StatusCode, Json<ApiErrorResponse>)> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let Some(resolver) = state.equipment_links() else {
        return Err(lot_dependency_unavailable());
    };
    match resolver.resolve_public_equipment_links(targets).await {
        Ok(links) if links.len() == targets.len() => Ok(links
            .into_iter()
            .map(auction_application::public::PublicEquipmentGameLink::into_url)
            .collect()),
        diagnostic_outcome => {
            let (phase, safe_kind) = match &diagnostic_outcome {
                Err(cause) => ("read_equipment_links", cause.diagnostic_kind()),
                Ok(_) => ("validate_equipment_links", SafeErrorKind::DEPENDENCY_MISMATCH),
            };
            diagnostics::warn("public_catalog", phase, safe_kind, DiagnosticContext::default());
            drop(diagnostic_outcome);
            Err(lot_dependency_unavailable())
        }
    }
}

fn lot_dependency_unavailable() -> (StatusCode, Json<ApiErrorResponse>) {
    api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use auction_application::auction::{
        PUBLIC_LOT_CATEGORIES, PublicAuctionProjection, PublicEquipment, PublicEquipmentStat,
        PublicFinalRestriction, PublicLiveLotDetail, PublicLiveLotSummary, PublicLotDetail,
        PublicLotListRequest, PublicLotPage, PublicLotReader, PublicPreviewLotDetail,
        PublicPreviewLotSummary,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::forum::EquipmentGroup;
    use auction_application::public::{
        PublicEquipmentGameLink, PublicEquipmentLinkResolver, PublicEquipmentLinkTarget,
    };
    use auction_domain::EquipmentId;
    use auction_time::{Duration, Timestamp};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{list_lots, lot_detail};
    use crate::middleware::security::PublicHttpSecurity;
    use crate::server::contract_fixtures::{assert_http, assert_wire};
    use crate::state::PublicApiState;

    const AUCTION_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b);
    const LOT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2c);
    const EQUIPMENT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2d);
    const STAFF_LOT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2e);
    const STAFF_EQUIPMENT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2f);
    const PROJECTED_URL: &str = "https://hentaiverse.org/equip/123456";

    /// 返回固定投影，并核对处理器确实把请求中的分类集合传到了读端口。
    struct FixedLots {
        page: PublicLotPage,
        expected_category_keys: Vec<String>,
    }

    impl FixedLots {
        fn new(page: PublicLotPage) -> Self {
            Self { page, expected_category_keys: Vec::new() }
        }

        fn expecting(page: PublicLotPage, expected_category_keys: &[&str]) -> Self {
            Self {
                page,
                expected_category_keys: expected_category_keys
                    .iter()
                    .map(|key| (*key).to_owned())
                    .collect(),
            }
        }
    }

    impl PublicLotReader for FixedLots {
        fn list_public_lots(
            &self,
            _auction_id: Uuid,
            request: PublicLotListRequest,
        ) -> ApplicationFuture<'_, Option<PublicLotPage>> {
            let received_category_keys: Vec<String> = request
                .category_selection
                .as_slice()
                .iter()
                .map(|category| category.public_key().to_owned())
                .collect();
            assert_eq!(
                received_category_keys, self.expected_category_keys,
                "读端口收到的分类集合不一致"
            );
            let page = self.page.clone();
            Box::pin(async move { Ok(Some(page)) })
        }

        fn public_lot(&self, lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>> {
            let page = self.page.clone();
            Box::pin(async move {
                let live = page.live.into_iter().find(|row| row.id == lot_id);
                let preview = page.preview.into_iter().find(|row| row.id == lot_id);
                let live_equipment = || live_page().live.remove(0).equipment;
                Ok(Some(PublicLotDetail {
                    equipment_id: EquipmentId::from_uuid(EQUIPMENT_ID),
                    projection: page.projection,
                    live: live.map(|summary| PublicLiveLotDetail { summary }),
                    preview: preview.map(|summary| PublicPreviewLotDetail {
                        summary,
                        equipment: live_equipment(),
                    }),
                }))
            })
        }
    }

    /// 为每个目标补上固定 key，模拟权威解析器返回的完整游戏链接。
    struct KeyedLinks;

    impl PublicEquipmentLinkResolver for KeyedLinks {
        fn resolve_public_equipment_links<'a>(
            &'a self,
            targets: &'a [PublicEquipmentLinkTarget],
        ) -> ApplicationFuture<'a, Vec<PublicEquipmentGameLink>> {
            Box::pin(async move {
                Ok(targets
                    .iter()
                    .map(|target| {
                        PublicEquipmentGameLink::new(format!(
                            "{}/0123456789",
                            target.projected_game_equipment_url
                        ))
                        .expect("fixture link")
                    })
                    .collect())
            })
        }
    }

    fn opens_at() -> Timestamp {
        Timestamp::from_timestamp(1_763_200_000, 0).expect("fixture time")
    }

    fn page(projection: PublicAuctionProjection) -> PublicLotPage {
        let mut category_counts = PUBLIC_LOT_CATEGORIES
            .iter()
            .map(|key| ((*key).to_owned(), 0))
            .collect::<BTreeMap<_, _>>();
        category_counts.insert("all".to_owned(), 1);
        PublicLotPage {
            projection,
            preview: Vec::new(),
            live: Vec::new(),
            total: 1,
            limit: 20,
            offset: 0,
            category_counts,
        }
    }

    fn projection(auction_status: &str) -> PublicAuctionProjection {
        let preview = auction_status == "preview";
        PublicAuctionProjection {
            auction_id: AUCTION_ID,
            world_id: "main".to_owned(),
            sequence: 5,
            auction_status: auction_status.to_owned(),
            projection_kind: if preview { "preview" } else { "live" }.to_owned(),
            presentation_mode: if preview { "preview" } else { "live" }.to_owned(),
            bidding_open: !preview,
            opens_at: opens_at(),
            scheduled_ends_at: (!preview).then(|| opens_at() + Duration::days(2)),
            preview_revision: 0,
            final_restriction: None,
        }
    }

    fn stat(value: &str, base: Option<f64>, roll: Option<f64>) -> PublicEquipmentStat {
        PublicEquipmentStat { value: value.to_owned(), base, roll }
    }

    fn live_page() -> PublicLotPage {
        let mut page = page(projection("live"));
        page.live.push(PublicLiveLotSummary {
            id: LOT_ID,
            equipment_id: EquipmentId::from_uuid(EQUIPMENT_ID),
            auction_id: AUCTION_ID,
            lot_number: 1,
            status: "live".to_owned(),
            seller_name: "seller".to_owned(),
            start_price: 1_000,
            current_price: 1_500,
            next_bid: 1_600,
            has_leader: true,
            leader_name: Some("leader".to_owned()),
            effective_end_at: Some(opens_at() + Duration::days(1)),
            state_version: 3,
            equipment: PublicEquipment {
                name: "Legendary Cotton Robe of Heimdall".to_owned(),
                category: "Cloth".to_owned(),
                equipment_group: EquipmentGroup::Rare,
                level: Some(450),
                info_summary: "Robe Lv450".to_owned(),
                stats: BTreeMap::from([
                    (
                        "Primary Attributes / Intelligence".to_owned(),
                        stat("+12.5", Some(10.0), Some(80.0)),
                    ),
                    ("Primary Attributes / Wisdom".to_owned(), stat("+9", Some(7.0), Some(60.0))),
                    ("Holy EDB".to_owned(), stat("+20%", None, None)),
                ]),
                avg_roll: Some(70.0),
                game_equipment_url: PROJECTED_URL.to_owned(),
                attributes: json!({}),
            },
        });
        page
    }

    fn live_lot(
        lot_id: Uuid,
        equipment_id: Uuid,
        lot_number: i64,
        category: &str,
        name: &str,
    ) -> PublicLiveLotSummary {
        PublicLiveLotSummary {
            id: lot_id,
            equipment_id: EquipmentId::from_uuid(equipment_id),
            auction_id: AUCTION_ID,
            lot_number,
            status: "live".to_owned(),
            seller_name: "seller".to_owned(),
            start_price: 1_000,
            current_price: 1_500,
            next_bid: 1_600,
            has_leader: true,
            leader_name: Some("leader".to_owned()),
            effective_end_at: Some(opens_at() + Duration::days(1)),
            state_version: 3,
            equipment: PublicEquipment {
                name: name.to_owned(),
                category: category.to_owned(),
                equipment_group: EquipmentGroup::Rare,
                level: Some(450),
                info_summary: String::new(),
                stats: BTreeMap::new(),
                avg_roll: None,
                game_equipment_url: PROJECTED_URL.to_owned(),
                attributes: json!({}),
            },
        }
    }

    fn multi_category_live_page() -> PublicLotPage {
        let mut page = page(projection("live"));
        page.total = 2;
        page.category_counts.insert("all".to_owned(), 3);
        page.category_counts.insert("one".to_owned(), 1);
        page.category_counts.insert("sta".to_owned(), 1);
        page.category_counts.insert("clo".to_owned(), 1);
        page.live = vec![
            live_lot(LOT_ID, EQUIPMENT_ID, 1, "One-Handed", "One-Handed fixture"),
            live_lot(STAFF_LOT_ID, STAFF_EQUIPMENT_ID, 2, "Staff", "Staff fixture"),
        ];
        page
    }

    fn preview_page() -> PublicLotPage {
        let mut page = page(projection("preview"));
        page.preview.push(PublicPreviewLotSummary {
            id: LOT_ID,
            equipment_id: EquipmentId::from_uuid(EQUIPMENT_ID),
            auction_id: AUCTION_ID,
            code: "Clo01".to_owned(),
            status: "pending".to_owned(),
            seller_name: "seller".to_owned(),
            start_price: 1_000,
            equipment_name: "Legendary Cotton Robe of Heimdall".to_owned(),
            equipment_category: "Cloth".to_owned(),
            equipment_group: EquipmentGroup::Rare,
            equipment_level: Some(450),
            primary_attribute_initials: Some("IW".to_owned()),
            avg_roll: Some(70.0),
            game_equipment_url: PROJECTED_URL.to_owned(),
        });
        page
    }

    fn state(lots: FixedLots) -> PublicApiState {
        PublicApiState::new(
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(lots)),
            Some(Arc::new(KeyedLinks)),
            None,
            None,
            None,
            None,
            None,
            None,
            PublicHttpSecurity::default(),
        )
    }

    async fn request_with_lots(lots: FixedLots, query: &str) -> (StatusCode, Vec<u8>) {
        let app = Router::new()
            .route("/api/v1/auctions/{auction_id}/lots", get(list_lots))
            .with_state(state(lots));
        let separator = if query.is_empty() { "" } else { "?" };
        let uri = format!("/api/v1/auctions/{AUCTION_ID}/lots{separator}{query}");
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        (status, body.to_vec())
    }

    async fn request(page: PublicLotPage, query: &str) -> (StatusCode, Vec<u8>) {
        request_with_lots(FixedLots::new(page), query).await
    }

    async fn request_json_with_lots(lots: FixedLots, query: &str) -> (StatusCode, Value) {
        let (status, body) = request_with_lots(lots, query).await;
        (status, serde_json::from_slice(&body).expect("json body"))
    }

    async fn request_json(page: PublicLotPage, query: &str) -> (StatusCode, Value) {
        request_json_with_lots(FixedLots::new(page), query).await
    }

    #[tokio::test]
    async fn summary_fields_drop_only_live_equipment_details() {
        let (status, full) = request_json(live_page(), "").await;
        assert_eq!(status, StatusCode::OK);
        let full_equipment = &full["data"]["lots"][0]["equipment"];
        // 回归：不带 fields 时仍是 v1 原字段集合，属性明细照常输出。
        let mut full_keys = full_equipment
            .as_object()
            .expect("equipment object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        full_keys.sort_unstable();
        assert_eq!(
            full_keys,
            [
                "avg_roll",
                "category",
                "equipment_group",
                "game_equipment_url",
                "info_summary",
                "level",
                "name",
                "primary_attribute_initials",
                "stats",
            ]
        );
        assert_eq!(full_equipment["info_summary"], "Robe Lv450");
        assert_eq!(full_equipment["stats"].as_object().map(serde_json::Map::len), Some(3));

        let (status, summary) = request_json(live_page(), "fields=summary").await;
        assert_eq!(status, StatusCode::OK);
        let summary_equipment = &summary["data"]["lots"][0]["equipment"];
        assert!(summary_equipment.get("stats").is_none(), "{summary_equipment}");
        assert!(summary_equipment.get("info_summary").is_none(), "{summary_equipment}");
        // 首字母与 avg_roll 在清空前已由 stats 推导，完整链接也不受投影影响。
        assert_eq!(summary_equipment["primary_attribute_initials"], "IW");
        assert_eq!(summary_equipment["avg_roll"], 70.0);
        assert_eq!(
            summary_equipment["game_equipment_url"],
            "https://hentaiverse.org/equip/123456/0123456789"
        );
        // 除两项属性明细外，精简投影与完整投影逐字段相同。
        let mut expected = full.clone();
        let equipment =
            expected["data"]["lots"][0]["equipment"].as_object_mut().expect("equipment object");
        equipment.remove("stats");
        equipment.remove("info_summary");
        assert_eq!(summary, expected);
    }

    #[tokio::test]
    async fn summary_fields_leave_preview_rows_unchanged() {
        let (status, full) = request(preview_page(), "").await;
        assert_eq!(status, StatusCode::OK);
        let (status, summary) = request(preview_page(), "fields=summary").await;
        assert_eq!(status, StatusCode::OK);
        // 预览行本来只有摘要字段；同一投影两种请求的正文逐字节一致。
        assert_eq!(summary, full);
    }

    #[tokio::test]
    async fn category_accepts_single_and_multiple_public_keys() {
        let mut single = live_page();
        single.category_counts.insert("clo".to_owned(), 1);
        let (status, body) =
            request_json_with_lots(FixedLots::expecting(single, &["clo"]), "category=clo").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["total"], 1);
        assert_eq!(body["data"]["category_counts"]["clo"], 1);

        let multiple = multi_category_live_page();
        for query in ["category=one,sta", "category=sta,one"] {
            let (status, body) = request_json_with_lots(
                FixedLots::expecting(multiple.clone(), &["one", "sta"]),
                query,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{query}");
            assert_eq!(body["data"]["total"], 2, "{query}");
            // 未选的 Cloth 计入 all，确认总数不是直接把 all 当作所选计数之和。
            assert_eq!(body["data"]["category_counts"]["all"], 3, "{query}");
            assert_eq!(body["data"]["category_counts"]["one"], 1, "{query}");
            assert_eq!(body["data"]["category_counts"]["sta"], 1, "{query}");
            assert_eq!(body["data"]["category_counts"]["clo"], 1, "{query}");
            let lots = body["data"]["lots"].as_array().expect("lots array");
            assert_eq!(lots.len(), 2, "{query}");
            assert_eq!(lots[0]["equipment"]["category"], "One-Handed", "{query}");
            assert_eq!(lots[1]["equipment"]["category"], "Staff", "{query}");
        }
    }

    #[tokio::test]
    async fn unselected_live_category_is_rejected_as_dependency_mismatch() {
        let mut page = page(projection("live"));
        page.total = 0;
        page.category_counts.insert("clo".to_owned(), 1);
        page.live = vec![live_lot(LOT_ID, EQUIPMENT_ID, 1, "Cloth", "Cloth fixture")];
        let (status, body) =
            request_json_with_lots(FixedLots::expecting(page, &["one", "sta"]), "category=one,sta")
                .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "temporarily_unavailable");
    }

    #[tokio::test]
    async fn invalid_category_lists_use_the_invalid_query_path() {
        for query in [
            "category=",
            "category=one,,sta",
            "category=,one",
            "category=one,",
            "category=all",
            "category=one,all",
            "category=one,one",
            "category=one,unknown",
            "category=one&category=sta",
        ] {
            let (status, body) = request_json(live_page(), query).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], "查询参数无效", "{query}");
        }
    }

    #[tokio::test]
    async fn unknown_or_repeated_fields_fail_closed() {
        for (query, message) in [
            ("fields=full", "字段投影无效"),
            ("fields=", "字段投影无效"),
            ("fields=SUMMARY", "字段投影无效"),
            ("fields=summary,stats", "字段投影无效"),
            ("fields=summary&fields=summary", "查询参数无效"),
        ] {
            let (status, body) = request_json(live_page(), query).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["code"], "invalid_query", "{query}");
            assert_eq!(body["message"], message, "{query}");
        }
    }

    #[tokio::test]
    async fn lot_read_contract_fixtures() {
        let mut final_page = live_page();
        final_page.projection.auction_status = "final".to_owned();
        final_page.projection.final_restriction = Some(PublicFinalRestriction {
            new_slow_proxy_allowed: false,
            existing_slow_proxy_converted: true,
        });
        final_page.live[0].state_version = 12;

        for (name, page, query) in [
            ("lots", live_page(), ""),
            ("lots-summary", live_page(), "fields=summary"),
            ("lots-preview", preview_page(), ""),
            ("lots-final", final_page, "fields=summary"),
        ] {
            let (status, body) = request_json(page, query).await;
            assert_eq!(status, StatusCode::OK, "{name}: {body}");
            assert_wire(&format!("public/{name}"), body);
        }
        let mut live = live_page();
        live.live[0].equipment.attributes = json!({
            "quality": "Legendary", "type": "Robe", "prefix": "Cotton", "level": 450
        });
        let mut unleveled = live.clone();
        unleveled.live[0].equipment.level = None;
        unleveled.live[0].equipment.info_summary = "Robe".to_owned();
        unleveled.live[0].equipment.attributes = json!({
            "quality": "Legendary", "type": "Robe", "prefix": "", "suffix": "", "level": null,
            "stats": {"Holy EDB": {"value": "+20%", "base": null}}
        });
        for (name, page) in
            [("lot", live), ("lot-unleveled", unleveled), ("lot-preview", preview_page())]
        {
            let state = state(FixedLots::new(page));
            let response = lot_detail(
                axum::extract::State(state),
                axum::extract::Path(LOT_ID.to_string()),
                "/".parse().expect("详情 URI"),
            )
            .await
            .expect("详情投影");
            assert_http(&format!("public/{name}"), StatusCode::OK, response).await;
        }
    }
}
