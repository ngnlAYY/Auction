//! 公开 lot 列表与详情的处理器。

use auction_application::auction::{MAX_PUBLIC_LOT_OFFSET, PublicLotListRequest};
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
    let category = query.get("category").cloned();
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
    let request = PublicLotListRequest {
        category,
        equipment_group,
        max_level,
        include_unleveled,
        limit: page_size,
        offset,
    };
    request.validate().map_err(|_| query_rejection())?;

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
    if page_result.validate(auction_id, query.get("category").map(String::as_str)).is_err()
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
        PublicLiveLotSummary, PublicLotDetail, PublicLotListRequest, PublicLotPage,
        PublicLotReader, PublicPreviewLotSummary,
    };
    use auction_application::core::ApplicationFuture;
    use auction_application::forum::EquipmentGroup;
    use auction_application::public::{
        PublicEquipmentGameLink, PublicEquipmentLinkResolver, PublicEquipmentLinkTarget,
    };
    use auction_domain::EquipmentId;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use chrono::{DateTime, Duration, Utc};
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::list_lots;
    use crate::middleware::security::PublicHttpSecurity;
    use crate::state::PublicApiState;

    const AUCTION_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b);
    const LOT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2c);
    const EQUIPMENT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2d);
    const PROJECTED_URL: &str = "https://hentaiverse.org/equip/123456";

    /// 原样返回固定投影；分页与筛选都按缺省请求构造，满足处理器的投影复核。
    struct FixedLots(PublicLotPage);

    impl PublicLotReader for FixedLots {
        fn list_public_lots(
            &self,
            _auction_id: Uuid,
            _request: PublicLotListRequest,
        ) -> ApplicationFuture<'_, Option<PublicLotPage>> {
            let page = self.0.clone();
            Box::pin(async move { Ok(Some(page)) })
        }

        fn public_lot(&self, _lot_id: Uuid) -> ApplicationFuture<'_, Option<PublicLotDetail>> {
            Box::pin(async { Ok(None) })
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

    fn opens_at() -> DateTime<Utc> {
        DateTime::from_timestamp(1_763_200_000, 0).expect("fixture time")
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

    async fn request(page: PublicLotPage, query: &str) -> (StatusCode, Vec<u8>) {
        let state = PublicApiState::new(
            None,
            None,
            None,
            None,
            None,
            None,
            Some(Arc::new(FixedLots(page))),
            Some(Arc::new(KeyedLinks)),
            None,
            None,
            None,
            None,
            None,
            None,
            PublicHttpSecurity::default(),
        );
        let app = Router::new()
            .route("/api/v1/auctions/{auction_id}/lots", get(list_lots))
            .with_state(state);
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

    async fn request_json(page: PublicLotPage, query: &str) -> (StatusCode, Value) {
        let (status, body) = request(page, query).await;
        (status, serde_json::from_slice(&body).expect("json body"))
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
}
