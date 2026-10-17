//! Admin 装备来源与拍品录入 HTTP 适配器。
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
    let (_, identity) = match super::authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    if !matches!(world_id.as_str(), "main" | "isekai") {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_world", "拍卖世界无效")
            .into_response();
    }
    let mut input: AdminIntakeRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let Some(command_id) = super::admin_auctions::parse_canonical_uuid(&input.command_id) else {
        return invalid_command();
    };
    let equipment_url = input.equipment_url.take().unwrap_or_default();
    let (source_kind, equipment, equipment_url) = match input.source_kind.as_deref() {
        None | Some("url") => {
            if equipment_url.is_empty() || input.equipment.is_some() {
                return invalid_command();
            }
            (Some(AdminIntakeSourceKind::Url), None, Some(equipment_url))
        }
        Some("json") => {
            if !equipment_url.is_empty() {
                return invalid_command();
            }
            let Some(equipment) = input.equipment.take().and_then(into_canonical_equipment) else {
                return invalid_command();
            };
            (Some(AdminIntakeSourceKind::Json), Some(equipment), None)
        }
        Some(_) => return invalid_command(),
    };
    let expected_start_price = input.start_price;
    let expected_player_id = input.player_id.trim().to_owned();
    let Some(port) = state.admin_intake() else {
        return feature_unavailable();
    };
    let result = match port
        .add_item(AdminIntakeCommand {
            command_id: CommandId::from_uuid(command_id),
            world_id,
            source_kind,
            equipment_url,
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
        Json(ApiResponse::created("拍品已加入最新一期拍卖", map_result(result))),
    )
        .into_response()
}

fn into_canonical_equipment(
    mut input: AdminCanonicalEquipmentRequest,
) -> Option<CanonicalEquipment> {
    let stats = input
        .stats
        .take()?
        .into_iter()
        .map(|(name, stat)| {
            Some((name, CanonicalEquipmentStat { value: stat.value?, base: stat.base }))
        })
        .collect::<Option<_>>()?;
    let equipment = CanonicalEquipment {
        external_equipment_id: input.external_equipment_id.take()?,
        name: input.name.take()?,
        category: input.category.take()?,
        equipment_type: input.equipment_type.take()?,
        quality: input.quality.take()?,
        prefix: input.prefix.take()?,
        suffix: input.suffix.take()?,
        level: input.level,
        tradeable: input.tradeable?,
        stats,
        key: input.key.take()?,
    };
    equipment.validate().is_ok().then_some(equipment)
}

fn invalid_command() -> Response {
    super::api_error(StatusCode::BAD_REQUEST, "invalid_auction_command", "后台拍品录入参数无效")
        .into_response()
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

fn mutation_boundary(uri: &Uri) -> Option<Response> {
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
            "装备 URL 或页面内容未通过安全校验",
        )
        .into_response(),
        ApplicationError::AdminIntake(AdminIntakeError::SourceUnavailable) => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "equipment_source_unavailable",
            "装备页面暂时无法读取；请检查对应世界的 MM Cookie 或稍后重试",
        )
        .into_response(),
        ApplicationError::AdminIntake(AdminIntakeError::SourceUnconfigured) => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "equipment_source_unavailable",
            "装备来源尚未配置",
        )
        .into_response(),
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(),
        ApplicationError::AdminIntake(AdminIntakeError::CommandConflict)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::AdminIntakeUnavailable
            | CompetitionTransactionError::NotFound,
        ) => super::api_error(
            StatusCode::CONFLICT,
            "auction_intake_unavailable",
            "当前世界没有可接收拍品的草稿场次；请先创建草稿场次",
        )
        .into_response(),
        ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::AdminIntakeSellerBlacklisted,
        ) => super::api_error(
            StatusCode::CONFLICT,
            "seller_blacklisted",
            "该玩家受黑名单限制；请解除限制或更换卖家",
        )
        .into_response(),
        ApplicationError::CompetitionTransaction(
            CompetitionTransactionError::AdminIntakeEquipmentConflict,
        ) => super::api_error(
            StatusCode::CONFLICT,
            "equipment_conflict",
            "该装备已存在于拍卖系统；请勿重复录入",
        )
        .into_response(),
        ApplicationError::AdminIntake(AdminIntakeError::KeyUnavailable)
        | ApplicationError::AdminIntake(AdminIntakeError::StoredResultCorrupt)
        | ApplicationError::Serialization(_) => temporarily_unavailable(),
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict) => {
            temporarily_unavailable()
        }
        _ => temporarily_unavailable(),
    }
}

fn temporarily_unavailable() -> Response {
    super::api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
        .into_response()
}
