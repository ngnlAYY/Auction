//! Public 补偿池 HTTP 适配器。
//! 这里仅处理旧版分页/游标契约与应用投影映射；补偿守恒、逐件结算与冲正一致性由应用端口及
//! PostgreSQL 快照读取边界负责。

use auction_application::core::ApplicationError;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_application::public::{
    DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE, DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE,
    MAX_PUBLIC_COMPENSATION_OFFSET, PublicCompensationCursor, PublicCompensationListRequest,
    PublicCompensationLot, PublicCompensationLotPage, PublicCompensationLotRequest,
    PublicCompensationMovement, PublicCompensationMovementPage, PublicCompensationMovementRequest,
    PublicCompensationPage, PublicCompensationRound, PublicEquipmentLinkTarget,
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
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use uuid::Uuid;

use super::{
    PublicApiState, api_error, not_found_error, parse_positive_i64, parse_public_uuid, parse_query,
    public_timestamp,
};
use crate::diagnostics;
use crate::middleware::response_cache::LOW_FREQUENCY_CACHE_CONTROL;

/// 读取已结算拍卖的补偿池批次。
pub(crate) async fn pool(
    State(state): State<PublicApiState>,
    uri: Uri,
) -> Result<
    (HeaderMap, Json<ApiResponse<PublicCompensationResponse>>),
    (StatusCode, Json<ApiErrorResponse>),
> {
    let query = parse_query(&uri, &["page", "page_size", "cursor"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    let page_size =
        parse_positive_i64(query.get("page_size"), DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE, None)
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;

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
        .map(decode_cursor)
        .transpose()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页游标无效"))?;
    let offset = if cursor.is_some() {
        0
    } else {
        page.checked_sub(1)
            .and_then(|value| value.checked_mul(page_size))
            .filter(|value| *value <= MAX_PUBLIC_COMPENSATION_OFFSET)
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
    let lots = map_lot_page(result, page, &equipment_urls).ok_or_else(|| {
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
    let query = parse_query(uri, &["page", "page_size"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    let page_size = parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE,
        None,
    )
    .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    let offset = page
        .checked_sub(1)
        .and_then(|value| value.checked_mul(page_size))
        .filter(|value| *value <= MAX_PUBLIC_COMPENSATION_OFFSET)
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
            page.total.is_none()
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
            .then(|| page.next_cursor.as_ref().map(encode_cursor).unwrap_or_default()),
        has_more: cursor.then_some(page.has_more),
    }
}

/// 投影已经校验，旧链接只供解析器核对；直接转移，避免逐行复制后再丢弃。
async fn resolve_movement_equipment_urls(
    state: &PublicApiState,
    page: &mut PublicCompensationMovementPage,
) -> Result<std::collections::HashMap<Uuid, String>, (StatusCode, Json<ApiErrorResponse>)> {
    let targets = page
        .entries
        .iter_mut()
        .filter_map(|movement| {
            Some(PublicEquipmentLinkTarget {
                lot_id: movement.lot_id?,
                equipment_id: movement.equipment_id?,
                world_id: page.round.world_id.clone(),
                projected_game_equipment_url: movement.projected_game_equipment_url.take()?,
            })
        })
        .collect::<Vec<_>>();
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
    Ok(targets
        .into_iter()
        .zip(links)
        .map(|(target, link)| (target.lot_id, link.into_url()))
        .collect())
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
fn map_lot_page(
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

fn encode_cursor(cursor: &PublicCompensationCursor) -> String {
    match cursor {
        PublicCompensationCursor::Start => "start".to_owned(),
        PublicCompensationCursor::Boundary { sequence, id } => {
            let mut payload = [0_u8; 26];
            payload[0] = 1;
            payload[1] = 6;
            payload[2..10].copy_from_slice(&sequence.to_be_bytes());
            payload[10..].copy_from_slice(id.as_bytes());
            URL_SAFE_NO_PAD.encode(payload)
        }
    }
}

fn decode_cursor(raw: &str) -> Result<PublicCompensationCursor, ()> {
    if raw == "start" {
        return Ok(PublicCompensationCursor::Start);
    }
    // 固定 26 字节只接受 35 个无填充字符；严格引擎校验规范尾位，无需分配解码/重编码副本。
    if raw.len() != 35 {
        return Err(());
    }
    let mut decoded = [0_u8; 26];
    let decoded_len = URL_SAFE_NO_PAD.decode_slice(raw, &mut decoded).map_err(|_| ())?;
    if decoded_len != decoded.len() || decoded[0] != 1 || decoded[1] != 6 {
        return Err(());
    }
    let sequence = i64::from_be_bytes(decoded[2..10].try_into().map_err(|_| ())?);
    let id = Uuid::from_slice(&decoded[10..]).map_err(|_| ())?;
    if sequence <= 0 || id.is_nil() {
        return Err(());
    }
    Ok(PublicCompensationCursor::Boundary { sequence, id })
}

fn map_reader_error(error: ApplicationError) -> (StatusCode, Json<ApiErrorResponse>) {
    if matches!(error, ApplicationError::CapabilityUnavailable(_)) {
        api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "补偿池仅支持 PostgreSQL 完整档",
        )
    } else {
        api_error(StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "补偿池暂不可用")
    }
}
