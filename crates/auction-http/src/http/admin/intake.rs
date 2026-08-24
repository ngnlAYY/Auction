//! Admin 装备来源与拍品录入 HTTP 适配器。
//!
//! 这里保持旧版的 URL/canonical JSON 请求和响应形状；来源读取、装备 key 处理、锁和幂等
//! 都交给应用端口与 PostgreSQL owner，处理器不直接拼接 SQL 或修改拍卖状态。

use std::net::SocketAddr;

use auction_application::{
    AdminIntakeCommand, AdminIntakeError, AdminIntakeSourceKind, ApplicationError,
    CanonicalEquipment, CanonicalEquipmentStat, CommandLedgerError, CompetitionTransactionError,
};
use auction_contracts::{
    AdminCanonicalEquipmentRequest, AdminIntakeRequest, AdminIntakeResponse, ApiResponse,
};
use auction_domain::CommandId;
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::AdminApiState;

/// Admin world-scoped 装备录入。
pub(crate) async fn add_item(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(world_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Some(response) = super::admin_session_guard(&state, &headers, address, true, false) {
        return response;
    }
    if let Some(response) = mutation_boundary(&headers, &uri) {
        return response;
    }
    if !matches!(world_id.as_str(), "main" | "isekai") {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_world", "拍卖世界无效")
            .into_response();
    }
    let (_, identity) = match super::authenticated_admin_session(&state, &headers).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    let mut input: AdminIntakeRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    if input.command_id.is_nil() {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    }
    let source_kind = match input.source_kind.as_deref() {
        None => None,
        Some("url") => Some(AdminIntakeSourceKind::Url),
        Some("json") => Some(AdminIntakeSourceKind::Json),
        Some(_) => {
            return super::api_error(
                StatusCode::BAD_REQUEST,
                "invalid_auction_intake",
                "后台拍品录入参数无效",
            )
            .into_response();
        }
    };
    let equipment = match source_kind {
        Some(AdminIntakeSourceKind::Json) => input.equipment.take().map(into_canonical_equipment),
        _ => None,
    };
    let expected_start_price = input.start_price;
    let expected_player_id = input.player_id.clone();
    let Some(port) = state.admin_intake() else {
        return feature_unavailable();
    };
    let result = match port
        .add_item(AdminIntakeCommand {
            command_id: CommandId::from_uuid(input.command_id),
            world_id,
            source_kind,
            equipment_url: input.equipment_url,
            equipment,
            start_price: input.start_price,
            player_id: input.player_id,
            actor_id: identity.actor,
            request_id: super::request_id(&headers),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if result.validate().is_err()
        || result.start_price != expected_start_price
        || result.seller_player_id != expected_player_id
    {
        return super::internal_error();
    }
    (
        StatusCode::CREATED,
        super::no_store_headers(),
        Json(ApiResponse::ok("拍品已加入最新一期拍卖", map_result(result))),
    )
        .into_response()
}

fn into_canonical_equipment(mut input: AdminCanonicalEquipmentRequest) -> CanonicalEquipment {
    let stats = std::mem::take(&mut input.stats)
        .into_iter()
        .map(|(name, stat)| (name, CanonicalEquipmentStat { value: stat.value, base: stat.base }))
        .collect();
    CanonicalEquipment {
        external_equipment_id: std::mem::take(&mut input.external_equipment_id),
        name: std::mem::take(&mut input.name),
        category: std::mem::take(&mut input.category),
        equipment_type: std::mem::take(&mut input.equipment_type),
        quality: std::mem::take(&mut input.quality),
        prefix: std::mem::take(&mut input.prefix),
        suffix: std::mem::take(&mut input.suffix),
        level: input.level,
        tradeable: input.tradeable,
        stats,
        key: std::mem::take(&mut input.key),
    }
}

fn map_result(result: auction_application::AdminIntakeResult) -> AdminIntakeResponse {
    AdminIntakeResponse {
        auction_id: result.auction_id,
        auction_sequence: result.auction_sequence,
        auction_state_version: result.auction_state_version,
        preview_revision: result.preview_revision,
        submission_id: result.submission_id,
        lot_id: result.lot_id,
        lot_number: result.lot_number,
        lot_state_version: result.lot_state_version,
        equipment_id: result.equipment_id,
        external_equipment_id: result.external_equipment_id,
        equipment_name: result.equipment_name,
        equipment_category: result.equipment_category,
        equipment_level: result.equipment_level,
        start_price: result.start_price,
        seller_player_id: result.seller_player_id,
    }
}

fn mutation_boundary(headers: &HeaderMap, uri: &Uri) -> Option<Response> {
    let Some(raw_csrf) = super::read_admin_cookie(headers, super::ADMIN_CSRF_COOKIE) else {
        return Some(super::authentication_error());
    };
    if !super::valid_admin_csrf_header(headers, &raw_csrf) {
        return Some(
            super::api_error(StatusCode::FORBIDDEN, "csrf_invalid", "CSRF 校验失败")
                .into_response(),
        );
    }
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return Some(
            super::api_error(StatusCode::BAD_REQUEST, "invalid_query", "拍品录入不接受查询参数")
                .into_response(),
        );
    }
    None
}

fn feature_unavailable() -> Response {
    super::api_error(
        StatusCode::NOT_IMPLEMENTED,
        "feature_not_supported",
        "Admin 装备录入仅支持 PostgreSQL 完整档",
    )
    .into_response()
}

fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminIntake(AdminIntakeError::InvalidCommand) => super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_auction_intake",
            "后台拍品录入参数无效",
        )
        .into_response(),
        ApplicationError::AdminIntake(AdminIntakeError::InvalidSource) => super::api_error(
            StatusCode::BAD_REQUEST,
            "unsupported_equipment_source",
            "装备来源不受支持",
        )
        .into_response(),
        ApplicationError::AdminIntake(AdminIntakeError::SourceUnavailable) => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "equipment_source_unavailable",
            "装备来源暂时不可读取",
        )
        .into_response(),
        ApplicationError::AdminIntake(AdminIntakeError::SourceUnconfigured)
        | ApplicationError::AdminIntake(AdminIntakeError::KeyUnavailable)
        | ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::AdminIntake(AdminIntakeError::CommandConflict)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::AdminIntake(AdminIntakeError::StoredResultCorrupt)
        | ApplicationError::Serialization(_) => super::internal_error(),
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict) => {
            super::api_error(
                StatusCode::CONFLICT,
                "auction_intake_unavailable",
                "当前世界没有可接收拍品的草稿场次，或录入目标与现有状态冲突",
            )
            .into_response()
        }
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "后台拍品录入暂不可用",
        )
        .into_response(),
    }
}
