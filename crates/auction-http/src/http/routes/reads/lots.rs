use auction_application::diagnostics::{DiagnosticContext, DiagnosticError, SafeErrorKind};

use crate::diagnostics;

use super::*;

/// 公开 lot 列表。preview 与 live 使用互斥的数组形状，保持旧系统的 JSON 契约。
pub(crate) async fn list_lots(
    State(state): State<PublicApiState>,
    Path(auction_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicLotEnvelopeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
    let auction_id = parse_public_uuid(&auction_id)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_auction_id", "拍卖 ID 无效"))?;
    let query = parse_query(
        &uri,
        &["category", "equipment_group", "max_level", "include_unleveled", "page", "page_size"],
    )
    .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page = parse_positive_i64(query.get("page"), 1, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let page_size = parse_positive_i64(query.get("page_size"), 20, None)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let offset = page
        .checked_sub(1)
        .and_then(|page| page.checked_mul(page_size))
        .filter(|offset| *offset <= MAX_PUBLIC_LOT_OFFSET)
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;
    let category = query.get("category").cloned();
    let equipment_group = query
        .get("equipment_group")
        .map(|value| {
            auction_application::EquipmentGroup::parse(value)
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
    let request = PublicLotListRequest {
        category,
        equipment_group,
        max_level,
        include_unleveled,
        limit: page_size,
        offset,
    };
    request
        .validate()
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效"))?;

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
    let response = map_lot_page(page_result, page, game_equipment_urls)
        .ok_or_else(lot_dependency_unavailable)?;
    Ok(Json(ApiResponse::ok("拍卖批次已加载", response)))
}

/// 公开 lot 详情只返回仍属于当前公开 projection 的对象。
pub(crate) async fn lot_detail(
    State(state): State<PublicApiState>,
    Path(lot_id): Path<String>,
    uri: Uri,
) -> Result<Json<ApiResponse<PublicLotEnvelopeResponse>>, (StatusCode, Json<ApiErrorResponse>)> {
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
    Ok(Json(ApiResponse::ok("拍卖批次已加载", response)))
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
        Ok(links) if links.len() == targets.len() => {
            Ok(links.into_iter().map(|link| link.into_url()).collect())
        }
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
