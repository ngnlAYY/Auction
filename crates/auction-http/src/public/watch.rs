//! 当前会话的装备标记边界；身份不来自请求负载，完整列表不允许静默截断。

use auction_application::core::diagnostics::DiagnosticError;

use crate::diagnostics;

use auction_application::public::{LotWatch, LotWatchError, MAX_LOT_WATCHES, SetLotWatchCommand};
use auction_contracts::{
    PublicLotWatchResponse, PublicLotWatchesResponse, PublicSetLotWatchRequest,
    PublicSetLotWatchResponse,
};

use crate::mapping::{api_error, parse_public_uuid, parse_query};
use crate::middleware::strict_json;
use crate::session::{
    authenticated_public_identity, authenticated_public_mutation_identity, no_store_headers,
    session_guard,
};
use crate::state::PublicApiState;
use auction_contracts::ApiResponse;
use auction_domain::CommandId;
use axum::Json;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;

pub(crate) async fn list_lot_watches(
    State(state): State<PublicApiState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let identity = match authenticated_public_identity(&state, &headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    let auction_id = parse_query(&uri, &["auction_id"])
        .ok()
        .and_then(|query| query.get("auction_id").and_then(|value| parse_public_uuid(value).ok()));
    let Some(auction_id) = auction_id else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    };
    let Some(port) = state.watches() else {
        return watch_error(LotWatchError::Unavailable);
    };
    let watches = match port.list(identity.user_id, auction_id).await {
        Ok(watches) => watches,
        Err(LotWatchError::Invalid) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效")
                .into_response();
        }
        Err(error) => return watch_error(error),
    };
    if watches.len() > MAX_LOT_WATCHES || watches.iter().any(|watch| !valid_watch(watch)) {
        return watch_error(LotWatchError::Unavailable);
    }
    let data = PublicLotWatchesResponse {
        user_id: identity.user_id.to_string(),
        auction_id: auction_id.to_string(),
        watches: watches.into_iter().map(map_watch).collect(),
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("装备标记已加载", data)))
        .into_response()
}

pub(crate) async fn set_lot_watch(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    if let Some(response) = session_guard(&state, &parts.headers, address, true, false) {
        return response;
    }
    let identity = match authenticated_public_mutation_identity(&state, &parts.headers).await {
        Ok(identity) => identity,
        Err(response) => return *response,
    };
    if parts.uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "查询参数无效").into_response();
    }
    let Ok(lot_id) = parse_public_uuid(&lot_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_lot_id", "拍卖批次 ID 无效")
            .into_response();
    };
    let input: PublicSetLotWatchRequest =
        match strict_json::decode_after_guard(&parts.headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Ok(command_id) = parse_public_uuid(&input.command_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    };
    if input.channel != "mm" || input.expected_version < 0 {
        return watch_error(LotWatchError::Invalid);
    }
    let Some(port) = state.watches() else {
        return watch_error(LotWatchError::Unavailable);
    };
    let result = match port
        .set(SetLotWatchCommand {
            command_id: CommandId::from_uuid(command_id),
            user_id: identity.user_id,
            lot_id,
            watched: input.watched,
            channel: input.channel,
            expected_version: input.expected_version,
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return watch_error(error),
    };
    if result.user_id != identity.user_id
        || result.auction_id.is_nil()
        || result.watch.lot_id != lot_id
        || result.watch.watched != input.watched
        || !valid_watch(&result.watch)
    {
        return watch_error(LotWatchError::Unavailable);
    }
    let data = PublicSetLotWatchResponse {
        user_id: result.user_id.to_string(),
        auction_id: result.auction_id.to_string(),
        watch: map_watch(result.watch),
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("装备标记已保存", data)))
        .into_response()
}

fn valid_watch(watch: &LotWatch) -> bool {
    !watch.lot_id.is_nil() && watch.version > 0 && watch.channel == "mm"
}

fn map_watch(watch: LotWatch) -> PublicLotWatchResponse {
    PublicLotWatchResponse {
        lot_id: watch.lot_id.to_string(),
        watched: watch.watched,
        channel: watch.channel,
        version: watch.version,
    }
}

fn watch_error(error: LotWatchError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || {
        let (status, code, message) = match error {
            LotWatchError::Invalid => {
                (StatusCode::UNPROCESSABLE_ENTITY, "invalid_watch", "标记输入无效或渠道未启用")
            }
            LotWatchError::Conflict => {
                (StatusCode::CONFLICT, "watch_conflict", "标记命令或版本冲突")
            }
            LotWatchError::NotFound => (StatusCode::NOT_FOUND, "not_found", "公开拍卖或标的不存在"),
            _ => (StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", "装备标记暂不可用"),
        };
        api_error(status, code, message).into_response()
    };
    let response = body();
    diagnostics::mapped_response("lot_watch", "respond_command_failure", diagnostic_kind, response)
}
