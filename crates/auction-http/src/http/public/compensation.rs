//! Public 补偿池 HTTP 适配器。
//!
//! 这里仅处理旧版分页/游标契约与应用投影映射；补偿守恒和账本完整性由应用端口及
//! PostgreSQL 快照读取边界负责。

use auction_application::{
    ApplicationError, DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE,
    DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE, MAX_PUBLIC_COMPENSATION_OFFSET,
    MAX_PUBLIC_COMPENSATION_PAGE_SIZE, PublicCompensationCursor, PublicCompensationListRequest,
    PublicCompensationMovement, PublicCompensationMovementPage, PublicCompensationMovementRequest,
    PublicCompensationPage, PublicCompensationRound,
};
use auction_contracts::{
    ApiErrorResponse, ApiResponse, PublicCompensationMovementPageResponse,
    PublicCompensationMovementResponse, PublicCompensationResponse,
    PublicCompensationRoundResponse,
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
    let page_size = parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PUBLIC_COMPENSATION_PAGE_SIZE,
        Some(MAX_PUBLIC_COMPENSATION_PAGE_SIZE),
    )
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
    let auction_id = parse_public_uuid(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let query = parse_query(&uri, &["page", "page_size"])
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    let page_size = parse_positive_i64(
        query.get("page_size"),
        DEFAULT_PUBLIC_COMPENSATION_MOVEMENT_PAGE_SIZE,
        Some(MAX_PUBLIC_COMPENSATION_PAGE_SIZE),
    )
    .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;
    let offset = page
        .checked_sub(1)
        .and_then(|value| value.checked_mul(page_size))
        .filter(|value| *value <= MAX_PUBLIC_COMPENSATION_OFFSET)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "分页参数无效"))?;

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
    let Some(result) = result else {
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

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((headers, Json(ApiResponse::ok("补偿池流水已加载", map_movement_page(result, page)))))
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
        updated_at: page.updated_at.map(public_timestamp),
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

fn map_movement_page(
    page: PublicCompensationMovementPage,
    page_number: i64,
) -> PublicCompensationMovementPageResponse {
    PublicCompensationMovementPageResponse {
        round: map_round(page.round),
        movements: page.entries.into_iter().map(map_movement).collect(),
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
        opening_balance: round.opening_balance,
        tax_income: round.tax_income,
        discount_demand: round.discount_demand,
        compensation_out: round.compensation_out,
        closing_balance: round.closing_balance,
        movement_count: round.movement_count,
        settled_at: public_timestamp(round.settled_at),
    }
}

fn map_movement(movement: PublicCompensationMovement) -> PublicCompensationMovementResponse {
    PublicCompensationMovementResponse {
        sequence: movement.sequence,
        lot_id: movement.lot_id.map(|value| value.to_string()),
        lot_number: movement.lot_number,
        equipment_name: movement.equipment_name,
        equipment_category: movement.equipment_category,
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
    if raw.is_empty() || raw.len() > 128 {
        return Err(());
    }
    let decoded = URL_SAFE_NO_PAD.decode(raw).map_err(|_| ())?;
    if decoded.len() != 26
        || URL_SAFE_NO_PAD.encode(&decoded) != raw
        || decoded[0] != 1
        || decoded[1] != 6
    {
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
