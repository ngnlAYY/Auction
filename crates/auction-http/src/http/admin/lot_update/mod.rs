//! Admin draft lot 修改 HTTP 适配器。
//! 这里只解析旧 HTTP 请求、会话和 CSRF，并把动作映射到应用命令；版本 fence、幂等、锁、
//! 预览修订和 withdrawn-return 义务全部由应用/存储层负责。

use std::net::SocketAddr;

use super::AdminApiState;
use auction_application::{
    AdminLotAction, AdminLotUpdateCommand, AdminLotUpdateError, ApplicationError,
    CompetitionTransactionError, RequestHash,
};
use auction_contracts::{AdminLotUpdateRequest, AdminLotUpdateResponse, ApiResponse};
use auction_domain::{CommandId, LotId};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

/// 执行 Admin draft lot 修改。
pub(crate) async fn update(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match super::authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return super::api_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "拍品修改不接受查询参数",
        )
        .into_response();
    }
    let Some(lot_uuid) = super::admin_auctions::parse_canonical_uuid(&lot_id) else {
        return super::api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍品 ID 无效")
            .into_response();
    };
    let input: AdminLotUpdateRequest = match serde_json::from_slice(&body) {
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
    let action = match input.action.as_str() {
        "withdraw" => AdminLotAction::Withdraw,
        "set_start_price" => AdminLotAction::SetStartPrice,
        _ => {
            return super::api_error(StatusCode::BAD_REQUEST, "invalid_lot_action", "拍品动作无效")
                .into_response();
        }
    };
    let request_hash = match RequestHash::from_request(action.command_kind(), lot_uuid, &input) {
        Ok(hash) => hash,
        Err(_) => return super::internal_error(),
    };
    let Some(port) = state.admin_lot_update() else {
        return super::api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "Admin lot 修改仅支持 PostgreSQL 完整档",
        )
        .into_response();
    };
    let command = AdminLotUpdateCommand {
        command_id: CommandId::from_uuid(input.command_id),
        lot_id: LotId::from_uuid(lot_uuid),
        expected_state_version: input.expected_state_version,
        action,
        start_price: input.start_price,
        actor_id: identity.actor,
        request_id: super::request_id(&headers),
        request_hash,
    };
    let result = match port.update(command).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate() || result.id != lot_uuid.into() || result.state_version <= 0 {
        return super::internal_error();
    }
    (
        StatusCode::OK,
        super::no_store_headers(),
        Json(ApiResponse::ok(
            "拍品已更新",
            AdminLotUpdateResponse { lot: super::admin_lots::map_lot(result) },
        )),
    )
        .into_response()
}

fn map_error(error: ApplicationError) -> Response {
    match error {
        ApplicationError::AdminLotUpdate(AdminLotUpdateError::InvalidCommand) => {
            super::api_error(StatusCode::BAD_REQUEST, "invalid_lot_command", "后台拍品修改参数无效")
                .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            super::not_found_error().into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::StaleState) => {
            super::api_error(StatusCode::CONFLICT, "stale_lot_state", "拍品状态版本已变化")
                .into_response()
        }
        ApplicationError::CompetitionTransaction(CompetitionTransactionError::InvalidState) => {
            super::api_error(StatusCode::CONFLICT, "invalid_lot_state", "拍品当前状态不允许修改")
                .into_response()
        }
        ApplicationError::CommandLedger(auction_application::CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            auction_application::CommandLedgerError::Conflict,
        )) => super::api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CapabilityUnavailable(_) => super::api_error(
            StatusCode::NOT_IMPLEMENTED,
            "feature_not_supported",
            "Admin lot 修改仅支持 PostgreSQL 完整档",
        )
        .into_response(),
        ApplicationError::Serialization(_) => super::internal_error(),
        _ => super::api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_unavailable",
            "Admin lot 服务暂不可用",
        )
        .into_response(),
    }
}
