//! Public 补偿池 HTTP 适配器。
//! 这里仅处理旧版分页/游标契约与应用投影映射；补偿守恒、逐件结算与冲正一致性由应用端口及
//! PostgreSQL 快照读取边界负责。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::public::{
    DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE, DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE,
    MAX_PUBLIC_COMPENSATION_OFFSET, PublicCompensationListRequest, PublicCompensationLot,
    PublicCompensationLotPage, PublicCompensationLotRequest, PublicCompensationMovement,
    PublicCompensationMovementPage, PublicCompensationMovementRequest, PublicCompensationPage,
    PublicCompensationRound, PublicEquipmentLinkTarget,
};
use auction_contracts::{
    ApiErrorResponse, ApiResponse, PublicCompensationLotPageResponse,
    PublicCompensationLotResponse, PublicCompensationLotReversalResponse,
    PublicCompensationMovementPageResponse, PublicCompensationMovementResponse,
    PublicCompensationResponse, PublicCompensationRoundResponse,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
};
use uuid::Uuid;

use crate::diagnostics;
use crate::errors::{api_error, not_found_error, query_rejection};
use crate::mapping::{
    decode_public_compensation_cursor, encode_public_compensation_cursor, public_timestamp,
};
use crate::middleware::response_cache::LOW_FREQUENCY_CACHE_CONTROL;
use crate::query::{
    page_offset, parse_positive_i64, parse_public_page_size, parse_public_uuid, parse_query,
};
use crate::state::PublicApiState;

/// 读取已结算拍卖的补偿池批次。
pub(crate) async fn pool(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<
    (HeaderMap, Json<ApiResponse<PublicCompensationResponse>>),
    (StatusCode, Json<ApiErrorResponse>),
> {
    let query =
        parse_query(&uri, &["page", "page_size", "cursor"]).map_err(|()| query_rejection())?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    let page_size =
        parse_public_page_size(query.get("page_size"), DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE)
            .map_err(|()| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;

    let raw_cursor = query.get("cursor").cloned();
    if raw_cursor.is_some() && page != 1 {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "游标分页只能从第一页开始",
        ));
    }
    let cursor = raw_cursor
        .as_deref()
        .map(decode_public_compensation_cursor)
        .transpose()
        .map_err(|()| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页游标无效"))?;
    let offset = if cursor.is_some() {
        0
    } else {
        page_offset(page, page_size, Some(MAX_PUBLIC_COMPENSATION_OFFSET))
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?
    };

    let Some(reader) = state.compensation() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "补偿池暂不可用",
        ));
    };
    let result = reader
        .list_public_compensation(PublicCompensationListRequest {
            limit: page_size,
            offset,
            cursor,
        })
        .await
        .map_err(map_reader_error)?;
    if !valid_pool_page(&result, page_size, offset, raw_cursor.is_some()) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "补偿池返回数据无效",
        ));
    }

    // 汇总带共用补偿池的当前余额，要随每次查看更新：可以保存，但使用前必须再验证。
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((
        headers,
        Json(ApiResponse::ok("补偿池批次已加载", map_pool_page(result, page, raw_cursor))),
    ))
}

/// 读取单场已结算拍卖的补偿账本。
pub(crate) async fn movements(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<
    (HeaderMap, Json<ApiResponse<PublicCompensationMovementPageResponse>>),
    (StatusCode, Json<ApiErrorResponse>),
> {
    let RoundPage { auction_id, page, page_size, offset } = parse_round_page(&auction_id, &uri)?;
    let Some(reader) = state.compensation() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "补偿账本暂不可用",
        ));
    };
    let result = reader
        .list_public_compensation_movements(
            auction_id,
            PublicCompensationMovementRequest { limit: page_size, offset },
        )
        .await
        .map_err(map_reader_error)?;
    let Some(mut result) = result else {
        return Err(not_found_error());
    };
    if result.round.auction_id != auction_id
        || result.limit != page_size
        || result.offset != offset
        || result.validate().is_err()
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "补偿账本返回数据无效",
        ));
    }

    let equipment_urls = resolve_movement_equipment_urls(&state, &mut result).await?;
    let mut headers = HeaderMap::new();
    // 流水可追加成交撤销冲正；使用分钟级缓存，避免长时间隐藏新冲正事实。
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(LOW_FREQUENCY_CACHE_CONTROL));
    Ok((
        headers,
        Json(ApiResponse::ok("补偿池流水已加载", map_movement_page(result, page, &equipment_urls))),
    ))
}

/// 读取单场已结算拍卖的逐件结算明细与成交撤销冲正。
pub(crate) async fn lots(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<
    (HeaderMap, Json<ApiResponse<PublicCompensationLotPageResponse>>),
    (StatusCode, Json<ApiErrorResponse>),
> {
    let RoundPage { auction_id, page, page_size, offset } = parse_round_page(&auction_id, &uri)?;
    let Some(reader) = state.compensation() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "逐件结算明细暂不可用",
        ));
    };
    let result = reader
        .list_public_compensation_lots(
            auction_id,
            PublicCompensationLotRequest { limit: page_size, offset },
        )
        .await
        .map_err(map_reader_error)?;
    let Some(mut result) = result else {
        return Err(not_found_error());
    };
    if result.round.auction_id != auction_id
        || result.limit != page_size
        || result.offset != offset
        || result.validate().is_err()
    {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "逐件结算明细返回数据无效",
        ));
    }

    // 投影已经校验，只供解析器核对；直接转移，避免逐行复制后再丢弃。
    let world_id = result.round.world_id.clone();
    let targets = result
        .entries
        .iter_mut()
        .map(|lot| PublicEquipmentLinkTarget {
            lot_id: lot.lot_id,
            equipment_id: lot.equipment_id,
            world_id: world_id.clone(),
            projected_game_equipment_url: std::mem::take(&mut lot.projected_game_equipment_url),
        })
        .collect::<Vec<_>>();
    let equipment_urls = resolve_equipment_urls(&state, targets).await?;
    let mut headers = HeaderMap::new();
    // 逐件明细同样可能被冲正更新；与流水一致使用分钟级缓存。
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(LOW_FREQUENCY_CACHE_CONTROL));
    let lots = map_compensation_lot_page(result, page, &equipment_urls).ok_or_else(|| {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "逐件结算明细返回数据无效",
        )
    })?;
    Ok((headers, Json(ApiResponse::ok("逐件结算明细已加载", lots))))
}

/// 单场明细共用的拍卖 ID 与编号分页参数。
struct RoundPage {
    auction_id: Uuid,
    page: i64,
    page_size: i64,
    offset: i64,
}

/// 解析单场明细共用的拍卖 ID 与编号分页参数。
fn parse_round_page(
    auction_id: &str,
    uri: &Uri,
) -> Result<RoundPage, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = parse_public_uuid(auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let query = parse_query(uri, &["page", "page_size"]).map_err(|()| query_rejection())?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    let page_size = parse_public_page_size(
        query.get("page_size"),
        DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE,
    )
    .map_err(|()| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    let offset = page_offset(page, page_size, Some(MAX_PUBLIC_COMPENSATION_OFFSET))
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    Ok(RoundPage { auction_id, page, page_size, offset })
}

fn valid_pool_page(
    page: &PublicCompensationPage,
    page_size: i64,
    offset: i64,
    cursor: bool,
) -> bool {
    page.validate().is_ok()
        && page.limit == page_size
        && page.offset == offset
        && if cursor {
            page.total.is_none() && page.has_more == page.next_cursor.is_some()
        } else {
            page.total.is_some() && page.next_cursor.is_none() && !page.has_more
        }
}

fn map_pool_page(
    page: PublicCompensationPage,
    page_number: i64,
    raw_cursor: Option<String>,
) -> PublicCompensationResponse {
    let cursor = raw_cursor.is_some();
    PublicCompensationResponse {
        balance: page.balance,
        updated_at: public_timestamp(page.updated_at),
        rounds: page.entries.into_iter().map(map_round).collect(),
        page: (!cursor).then_some(page_number),
        page_size: page.limit,
        total: if cursor { None } else { page.total },
        cursor: raw_cursor,
        next_cursor: cursor
            .then(|| page.next_cursor.as_ref().map_or_default(encode_public_compensation_cursor)),
        has_more: cursor.then_some(page.has_more),
    }
}

/// 投影已经校验，旧链接只供解析器核对；直接转移，避免逐行复制后再丢弃。
async fn resolve_movement_equipment_urls(
    state: &PublicApiState,
    page: &mut PublicCompensationMovementPage,
) -> Result<std::collections::HashMap<Uuid, String>, (StatusCode, Json<ApiErrorResponse>)> {
    let mut targets = Vec::new();
    for movement in &mut page.entries {
        let projected_game_equipment_url = movement.projected_game_equipment_url.take();
        match (movement.lot_id, movement.equipment_id, projected_game_equipment_url) {
            (None, None, None) => {}
            (Some(lot_id), Some(equipment_id), Some(projected_game_equipment_url)) => {
                targets.push(PublicEquipmentLinkTarget {
                    lot_id,
                    equipment_id,
                    world_id: page.round.world_id.clone(),
                    projected_game_equipment_url,
                });
            }
            _ => {
                return Err(api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "storage_unavailable",
                    "补偿池返回数据无效",
                ));
            }
        }
    }
    resolve_equipment_urls(state, targets).await
}

/// 经权威解析器把不含 key 的装备投影换成完整游戏链接；数量不符时失败关闭。
async fn resolve_equipment_urls(
    state: &PublicApiState,
    targets: Vec<PublicEquipmentLinkTarget>,
) -> Result<std::collections::HashMap<Uuid, String>, (StatusCode, Json<ApiErrorResponse>)> {
    if targets.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    if targets.iter().any(|target| target.validate().is_err()) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "补偿池返回数据无效",
        ));
    }
    let Some(resolver) = state.equipment_links() else {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "服务暂时不可用",
        ));
    };
    let links =
        resolver.resolve_public_equipment_links(&targets).await.map_err(|diagnostic_cause| {
            diagnostics::warn(
                "public_compensation",
                "resolve_equipment_links",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );

            api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
        })?;
    if links.len() != targets.len() {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "补偿池返回数据无效",
        ));
    }
    let mut urls = std::collections::HashMap::with_capacity(targets.len());
    for (target, link) in targets.into_iter().zip(links) {
        let url = link.into_url();
        let matches_target =
            url.strip_prefix(&target.projected_game_equipment_url).is_some_and(|suffix| {
                suffix.len() == 11
                    && suffix.starts_with('/')
                    && suffix[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
            });
        if !matches_target {
            return Err(api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "补偿池返回数据无效",
            ));
        }
        urls.insert(target.lot_id, url);
    }
    Ok(urls)
}

fn map_movement_page(
    page: PublicCompensationMovementPage,
    page_number: i64,
    equipment_urls: &std::collections::HashMap<Uuid, String>,
) -> PublicCompensationMovementPageResponse {
    PublicCompensationMovementPageResponse {
        round: map_round(page.round),
        movements: page
            .entries
            .into_iter()
            .map(|movement| {
                let url = movement.lot_id.and_then(|lot_id| equipment_urls.get(&lot_id).cloned());
                map_movement(movement, url)
            })
            .collect(),
        total: page.total,
        page: page_number,
        page_size: page.limit,
    }
}

fn map_round(round: PublicCompensationRound) -> PublicCompensationRoundResponse {
    PublicCompensationRoundResponse {
        auction_id: round.auction_id.to_string(),
        world_id: round.world_id,
        auction_sequence: round.auction_sequence,
        policy_mode: round.policy_mode.as_str().to_owned(),
        opening_balance: round.opening_balance,
        tax_income: round.tax_income,
        discount_demand: round.discount_demand,
        compensation_out: round.compensation_out,
        closing_balance: round.closing_balance,
        movement_count: round.movement_count,
        reversal_count: round.reversal_count,
        reversal_amount: round.reversal_amount,
        settled_at: public_timestamp(round.settled_at),
    }
}

/// 每件都必须取得完整装备链接；缺失时返回 `None` 由调用方失败关闭。
fn map_compensation_lot_page(
    page: PublicCompensationLotPage,
    page_number: i64,
    equipment_urls: &std::collections::HashMap<Uuid, String>,
) -> Option<PublicCompensationLotPageResponse> {
    let lots = page
        .entries
        .into_iter()
        .map(|lot| {
            let url = equipment_urls.get(&lot.lot_id).cloned()?;
            Some(map_lot(lot, url))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(PublicCompensationLotPageResponse {
        round: map_round(page.round),
        lots,
        total: page.total,
        page: page_number,
        page_size: page.limit,
    })
}

fn map_lot(
    lot: PublicCompensationLot,
    game_equipment_url: String,
) -> PublicCompensationLotResponse {
    PublicCompensationLotResponse {
        lot_id: lot.lot_id.to_string(),
        lot_number: lot.lot_number,
        equipment_name: lot.equipment_name,
        equipment_category: lot.equipment_category,
        game_equipment_url,
        end_price: lot.end_price,
        hold_ppm: lot.hold_ppm,
        adjustment_ppm: lot.adjustment_ppm,
        leader_held_nanos: lot.leader_held_nanos,
        tax_amount: lot.tax_amount,
        discount_amount: lot.discount_amount,
        compensation_amount: lot.compensation_amount,
        buyer_amount: lot.buyer_amount,
        seller_amount: lot.seller_amount,
        buyer_name: lot.buyer_name,
        seller_name: lot.seller_name,
        voided: lot.voided,
        reversal: lot.reversal.map(|reversal| PublicCompensationLotReversalResponse {
            amount: reversal.amount,
            tax_shortfall: reversal.tax_shortfall,
            balance_after: reversal.balance_after,
            reversed_at: public_timestamp(reversal.created_at),
        }),
    }
}

fn map_movement(
    movement: PublicCompensationMovement,
    game_equipment_url: Option<String>,
) -> PublicCompensationMovementResponse {
    PublicCompensationMovementResponse {
        sequence: movement.sequence,
        lot_id: movement.lot_id.map(|value| value.to_string()),
        lot_number: movement.lot_number,
        equipment_name: movement.equipment_name,
        equipment_category: movement.equipment_category,
        game_equipment_url,
        hold_ppm: movement.hold_ppm,
        adjustment_ppm: movement.adjustment_ppm,
        leader_held_nanos: movement.leader_held_nanos,
        kind: movement.kind.as_str().to_owned(),
        amount: movement.amount,
        balance_after: movement.balance_after,
        created_at: public_timestamp(movement.created_at),
    }
}

/// 读端口只有数据库实现，任何读取失败都按依赖不可用折叠；折叠前先投影安全分类。
fn map_reader_error(error: ApplicationError) -> (StatusCode, Json<ApiErrorResponse>) {
    diagnostics::warn(
        "public_compensation",
        "read_compensation",
        DiagnosticError::diagnostic_kind(&error),
        DiagnosticContext::default(),
    );
    api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "补偿池暂不可用")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::core::{ApplicationError, ApplicationFuture};
    use auction_application::public::{
        PublicCompensationCursor, PublicCompensationListRequest, PublicCompensationLot,
        PublicCompensationLotPage, PublicCompensationLotRequest, PublicCompensationLotReversal,
        PublicCompensationMovement, PublicCompensationMovementKind, PublicCompensationMovementPage,
        PublicCompensationMovementRequest, PublicCompensationPage, PublicCompensationReader,
        PublicCompensationRound, PublicEquipmentGameLink, PublicEquipmentLinkResolver,
        PublicEquipmentLinkTarget,
    };
    use auction_domain::{EquipmentId, SettlementMode};
    use auction_time::Timestamp;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use uuid::Uuid;

    use super::{lots, map_reader_error, movements, pool};
    use crate::middleware::security::PublicHttpSecurity;
    use crate::server::contract_fixtures::assert_http;
    use crate::state::PublicApiState;

    #[test]
    fn reader_failures_collapse_to_storage_unavailable() {
        for error in [
            ApplicationError::CapabilityUnavailable("compensation"),
            ApplicationError::FulfillmentDependencyUnavailable,
        ] {
            let (status, body) = map_reader_error(error);
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body.0.code, "storage_unavailable");
        }
    }

    const AUCTION_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b);
    const LOT_ID: Uuid = Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2c);
    const EQUIPMENT_ID: Uuid = Uuid::from_u128(0x4000_8000_0000_0000_0017);
    const EQUIPMENT_PREFIX: &str = "https://hentaiverse.org/equip/123";

    fn round(voided: bool) -> PublicCompensationRound {
        PublicCompensationRound {
            auction_id: AUCTION_ID,
            world_id: "main".to_owned(),
            auction_sequence: 5,
            policy_mode: SettlementMode::HoldCurveV2,
            opening_balance: 1_000,
            tax_income: 125,
            discount_demand: 100,
            compensation_out: 100,
            closing_balance: 1_025,
            movement_count: 4,
            reversal_count: i64::from(voided),
            reversal_amount: if voided { -25 } else { 0 },
            settled_at: Timestamp::from_timestamp(1_763_200_000, 0).expect("结算时刻"),
        }
    }

    struct FixtureCompensation {
        voided: bool,
        held_nanos: i64,
    }

    impl PublicCompensationReader for FixtureCompensation {
        fn list_public_compensation(
            &self,
            request: PublicCompensationListRequest,
        ) -> ApplicationFuture<'_, PublicCompensationPage> {
            let round = round(self.voided);
            let cursor_mode = request.cursor.is_some();
            Box::pin(async move {
                Ok(PublicCompensationPage {
                    balance: round.closing_balance + round.reversal_amount,
                    updated_at: round.settled_at,
                    total: (!cursor_mode).then_some(1),
                    limit: request.limit,
                    offset: request.offset,
                    next_cursor: cursor_mode.then_some(PublicCompensationCursor::Boundary {
                        sequence: round.auction_sequence,
                        id: round.auction_id,
                    }),
                    has_more: cursor_mode,
                    entries: vec![round],
                })
            })
        }

        fn list_public_compensation_lots(
            &self,
            auction_id: Uuid,
            request: PublicCompensationLotRequest,
        ) -> ApplicationFuture<'_, Option<PublicCompensationLotPage>> {
            assert_eq!(auction_id, AUCTION_ID);
            let voided = self.voided;
            let round = round(voided);
            let page = PublicCompensationLotPage {
                entries: vec![PublicCompensationLot {
                    lot_id: LOT_ID,
                    lot_number: 1,
                    equipment_name: "Legendary Staff".to_owned(),
                    equipment_category: "Staff".to_owned(),
                    equipment_id: EquipmentId::from_uuid(EQUIPMENT_ID),
                    projected_game_equipment_url: EQUIPMENT_PREFIX.to_owned(),
                    end_price: 2_000,
                    hold_ppm: 500_000,
                    adjustment_ppm: 50_000,
                    leader_held_nanos: self.held_nanos,
                    tax_amount: 125,
                    discount_amount: 100,
                    compensation_amount: 100,
                    buyer_amount: 2_025,
                    seller_amount: 2_000,
                    buyer_name: "ExampleBuyer".to_owned(),
                    seller_name: "ExampleSeller".to_owned(),
                    voided,
                    reversal: voided.then_some(PublicCompensationLotReversal {
                        amount: -25,
                        tax_shortfall: 0,
                        balance_after: 1_000,
                        created_at: round.settled_at + auction_time::Duration::days(1),
                    }),
                }],
                round,
                total: 1,
                limit: request.limit,
                offset: request.offset,
            };
            Box::pin(async move { Ok(Some(page)) })
        }

        fn list_public_compensation_movements(
            &self,
            auction_id: Uuid,
            request: PublicCompensationMovementRequest,
        ) -> ApplicationFuture<'_, Option<PublicCompensationMovementPage>> {
            use PublicCompensationMovementKind::{Closing, Compensation, Opening, Tax};

            assert_eq!(auction_id, AUCTION_ID);
            let round = round(self.voided);
            let entries = [
                (1, Opening, 1_000, 1_000),
                (2, Tax, 125, 1_125),
                (3, Compensation, -100, 1_025),
                (4, Closing, 0, 1_025),
            ]
            .into_iter()
            .map(|(sequence, kind, amount, balance_after)| {
                let has_lot = matches!(kind, Tax | Compensation);
                PublicCompensationMovement {
                    auction_id: AUCTION_ID,
                    sequence,
                    lot_id: has_lot.then_some(LOT_ID),
                    lot_number: has_lot.then_some(1),
                    equipment_name: has_lot.then(|| "Legendary Staff".to_owned()),
                    equipment_category: has_lot.then(|| "Staff".to_owned()),
                    equipment_id: has_lot.then(|| EquipmentId::from_uuid(EQUIPMENT_ID)),
                    projected_game_equipment_url: has_lot.then(|| EQUIPMENT_PREFIX.to_owned()),
                    hold_ppm: has_lot.then_some(500_000),
                    adjustment_ppm: has_lot.then_some(50_000),
                    leader_held_nanos: has_lot.then_some(self.held_nanos),
                    kind,
                    amount,
                    balance_after,
                    created_at: round.settled_at,
                }
            })
            .collect();
            Box::pin(async move {
                Ok(Some(PublicCompensationMovementPage {
                    round,
                    entries,
                    total: 4,
                    limit: request.limit,
                    offset: request.offset,
                }))
            })
        }
    }

    struct FixtureEquipmentLinks;

    impl PublicEquipmentLinkResolver for FixtureEquipmentLinks {
        fn resolve_public_equipment_links<'a>(
            &'a self,
            targets: &'a [PublicEquipmentLinkTarget],
        ) -> ApplicationFuture<'a, Vec<PublicEquipmentGameLink>> {
            Box::pin(async move {
                Ok(targets
                    .iter()
                    .map(|target| {
                        assert_eq!(target.lot_id, LOT_ID);
                        PublicEquipmentGameLink::new(format!(
                            "{}/0123456789",
                            target.projected_game_equipment_url
                        ))
                        .expect("样本装备链接")
                    })
                    .collect())
            })
        }
    }

    fn state(voided: bool, held_nanos: i64) -> State<PublicApiState> {
        State(PublicApiState::new(
            None,
            None,
            None,
            Some(Arc::new(FixtureCompensation { voided, held_nanos })),
            None,
            None,
            None,
            Some(Arc::new(FixtureEquipmentLinks)),
            None,
            None,
            None,
            None,
            None,
            None,
            PublicHttpSecurity::default(),
        ))
    }

    #[tokio::test]
    async fn compensation_contract_fixtures() {
        const ONE_HOUR_NANOS: i64 = 3_600_000_000_000;
        for (name, url, voided) in [
            ("public/compensation-pool", "/api/v1/compensation-pool", false),
            ("public/compensation-pool-reversed", "/api/v1/compensation-pool", true),
            (
                "public/compensation-pool-cursor",
                "/api/v1/compensation-pool?cursor=start&page_size=1",
                false,
            ),
        ] {
            let response = pool(state(voided, ONE_HOUR_NANOS), url.parse().expect("补偿池 URI"))
                .await
                .into_response();
            assert_http(name, StatusCode::OK, response).await;
        }

        let lots_url = format!("/api/v1/compensation-pool/{AUCTION_ID}/lots");
        for (name, voided, held_nanos) in [
            ("public/compensation-lots", false, ONE_HOUR_NANOS),
            ("public/compensation-lots-reversed", true, 0),
            ("public/compensation-lots-long-hold", false, 9_072_000_000_000_000),
        ] {
            let response = lots(
                state(voided, held_nanos),
                Path(AUCTION_ID.to_string()),
                lots_url.parse().expect("逐件明细 URI"),
            )
            .await
            .into_response();
            assert_http(name, StatusCode::OK, response).await;
        }

        let movements_url = format!("/api/v1/compensation-pool/{AUCTION_ID}/movements");
        for (name, held_nanos) in [
            ("public/compensation-movements", ONE_HOUR_NANOS),
            ("public/compensation-movements-long-hold", 9_072_000_000_000_000),
        ] {
            let response = movements(
                state(false, held_nanos),
                Path(AUCTION_ID.to_string()),
                movements_url.parse().expect("流水 URI"),
            )
            .await
            .into_response();
            assert_http(name, StatusCode::OK, response).await;
        }
    }
}
