//! Public 代理创建与降价命令 handler。

use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};

use crate::diagnostics;

use crate::http::routes::state::PublicApiState;
use crate::http::routes::{
    api_error, competition_unavailable, internal_error, map_proxy_command_response,
    no_store_headers, not_found_error, parse_public_uuid, request_id, strict_json,
};
use auction_application::auction::{CreateProxyCommand, LowerProxyCommand};
use auction_contracts::{
    ApiResponse, CreateProxyAuthorizationRequest, LowerProxyAuthorizationRequest,
};
use auction_domain::{
    BidAmount, CommandId, DomainError, LotId, ProxyAuthorizationId, ProxyMode, UserId,
};
use auction_observability::BusinessEvent;
use axum::Json;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use uuid::Uuid;

/// 创建代理授权命令；命令账本和代理快照提交后返回旧契约要求的完整命令投影。
pub(crate) async fn create_proxy_authorization(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(lot_id): Path<String>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let (identity, lot_uuid) =
        match super::command_entry(&state, &headers, address, &parts.uri, &lot_id).await {
            Ok(entry) => entry,
            Err(response) => return *response,
        };
    let input: CreateProxyAuthorizationRequest =
        match strict_json::decode_after_guard(&headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Ok(command_uuid) = parse_public_uuid(&input.command_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    };
    if input.maximum_amount <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_maximum_amount", "代理上限无效")
            .into_response();
    }
    let mode = match input.mode.as_str() {
        "fast" => ProxyMode::Fast,
        "slow" => ProxyMode::Slow,
        _ => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_proxy_mode", "代理模式无效")
                .into_response();
        }
    };
    // 旧 v1 在正数网格校验前先确认 lot 对当前 Public projection 可见。
    if let Some(response) = require_visible_proxy_lot(&state, lot_uuid).await {
        return response;
    }
    let maximum_amount = match BidAmount::new(input.maximum_amount) {
        Ok(amount) => amount,
        // 兼容旧 v1 的两阶段分类：正数网格错误失败关闭为依赖不可用，不进入竞价事务。
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "服务暂时不可用",
            )
            .into_response();
        }
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "public_proxy",
                "read_create_lot",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let Some(commands) = state.commands() else {
        return competition_unavailable();
    };
    let command_id = CommandId::from_uuid(command_uuid);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let request_hash =
        match CreateProxyCommand::legacy_request_hash(lot_id, bidder_id, mode, maximum_amount) {
            Ok(request_hash) => request_hash,
            Err(diagnostic_cause) => {
                diagnostics::error(
                    "public_proxy",
                    "hash_create",
                    DiagnosticError::diagnostic_kind(&diagnostic_cause),
                    DiagnosticContext::default(),
                );
                return internal_error();
            }
        };
    let command = CreateProxyCommand {
        command_id,
        lot_id,
        bidder_id,
        maximum_amount,
        mode: Some(mode),
        request_hash,
        request_id: request_id(&headers),
    };
    let outcome =
        match super::observed_command(&state, BusinessEvent::Proxy, commands.create_proxy(command))
            .await
        {
            Ok(outcome) => outcome,
            Err(response) => return *response,
        };
    let effective_end_at = outcome.effective_end_at;
    let plan = outcome.plan;
    let Some(proxy) = outcome.proxy else {
        return internal_error();
    };
    let expected_state = proxy.authorization;
    if expected_state.lot_id != lot_id
        || expected_state.user_id != bidder_id
        || expected_state.creation_command_id != command_id
        || !plan.proxy_states.contains(&expected_state)
    {
        return internal_error();
    }
    let Some(data) = map_proxy_command_response(proxy, &plan, bidder_id, effective_end_at) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("代理授权已创建", data)))
        .into_response()
}

/// 降低代理授权上限命令；命令账本保证重试不会重复追加转换事实，并返回完整命令投影。
pub(crate) async fn lower_proxy_authorization(
    State(state): State<PublicApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path((lot_id, authorization_id)): Path<(String, String)>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let (identity, lot_uuid) =
        match super::command_entry(&state, &headers, address, &parts.uri, &lot_id).await {
            Ok(entry) => entry,
            Err(response) => return *response,
        };
    let Ok(authorization_uuid) = parse_public_uuid(&authorization_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_authorization_id", "代理授权 ID 无效")
            .into_response();
    };
    let input: LowerProxyAuthorizationRequest =
        match strict_json::decode_after_guard(&headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    let Ok(command_uuid) = parse_public_uuid(&input.command_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_command_id", "命令 ID 无效")
            .into_response();
    };
    if input.maximum_amount <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_maximum_amount", "代理上限无效")
            .into_response();
    }
    // lower 与创建代理保持相同的旧边界：隐藏 lot 优先于正数但非信用网格的金额。
    if let Some(response) = require_visible_proxy_lot(&state, lot_uuid).await {
        return response;
    }
    let new_maximum = match BidAmount::new(input.maximum_amount) {
        Ok(amount) => amount,
        // 降低上限沿用代理创建的旧 wire 分类，且不因兼容外壳放宽领域网格。
        Err(DomainError::InvalidAmount { .. }) => {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "服务暂时不可用",
            )
            .into_response();
        }
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "public_proxy",
                "read_lower_lot",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let Some(commands) = state.commands() else {
        return competition_unavailable();
    };
    let command_id = CommandId::from_uuid(command_uuid);
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(identity.user_id);
    let authorization_id = ProxyAuthorizationId::from_uuid(authorization_uuid);
    let request_hash = match LowerProxyCommand::legacy_request_hash(
        lot_id,
        bidder_id,
        authorization_id,
        new_maximum,
    ) {
        Ok(request_hash) => request_hash,
        Err(diagnostic_cause) => {
            diagnostics::error(
                "public_proxy",
                "hash_lower",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let command = LowerProxyCommand {
        command_id,
        lot_id,
        bidder_id,
        authorization_id,
        new_maximum,
        request_hash,
        request_id: request_id(&headers),
    };
    let outcome =
        match super::observed_command(&state, BusinessEvent::Proxy, commands.lower_proxy(command))
            .await
        {
            Ok(outcome) => outcome,
            Err(response) => return *response,
        };
    let effective_end_at = outcome.effective_end_at;
    let plan = outcome.plan;
    let Some(proxy) = outcome.proxy else {
        return internal_error();
    };
    let expected_state = proxy.authorization;
    if expected_state.id != authorization_id
        || expected_state.lot_id != lot_id
        || expected_state.user_id != bidder_id
        || !plan.proxy_states.contains(&expected_state)
        || plan.final_lot.lot_id != lot_id
        || !plan.proxy_transitions.iter().any(|transition| {
            transition.authorization_id == authorization_id && transition.command_id == command_id
        })
    {
        return internal_error();
    }
    let Some(data) = map_proxy_command_response(proxy, &plan, bidder_id, effective_end_at) else {
        return internal_error();
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("代理授权上限已降低", data)))
        .into_response()
}

/// 旧 v1 只允许对当前公开 projection 中可见的 lot 创建或降低代理授权。
async fn require_visible_proxy_lot(state: &PublicApiState, lot_id: Uuid) -> Option<Response> {
    let Some(reader) = state.lots() else {
        return Some(
            api_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable", "服务暂时不可用")
                .into_response(),
        );
    };
    match reader.public_lot(lot_id).await {
        Ok(Some(_)) => None,
        Ok(None) => Some(not_found_error().into_response()),
        Err(diagnostic_cause) => {
            diagnostics::warn(
                "public_proxy",
                "read_visible_lot",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            Some(
                api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "temporarily_unavailable",
                    "服务暂时不可用",
                )
                .into_response(),
            )
        }
    }
}
