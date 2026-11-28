//! Admin 业务世界目录与启停 HTTP 适配器。

use std::net::SocketAddr;

use auction_application::admin::{AdminWorldError, AdminWorldSetEnabledCommand};
use auction_application::auction::WorldSnapshot;
use auction_application::core::diagnostics::DiagnosticError;
use auction_application::core::{
    ApplicationError, CommandLedgerError, CompetitionTransactionError,
};
use auction_contracts::{
    AdminWorldListResponse, AdminWorldResponse, AdminWorldSetEnabledRequest,
    AdminWorldSetEnabledResponse, ApiResponse,
};
use auction_domain::CommandId;
use axum::{
    Json,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use crate::diagnostics;
use crate::errors::{CAPABILITY_NOT_PROVIDED, api_error, feature_unavailable, storage_unavailable};
use crate::middleware::boundary::request_id;
use crate::middleware::strict_json;
use crate::query::parse_query;
use crate::session::{
    admin_session_guard, authenticated_admin_mutation, authenticated_admin_session, internal_error,
    no_store_headers,
};
use crate::state::AdminApiState;

pub(crate) async fn list(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if let Some(response) = admin_session_guard(&state, &headers, address, false, false) {
        return response;
    }
    if let Err(response) = authenticated_admin_session(&state, &headers).await {
        return *response;
    }
    if parse_query(&uri, &[]).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "业务世界目录不接受查询参数")
            .into_response();
    }
    let Some(port) = state.admin_worlds() else {
        return storage_unavailable("业务世界状态暂不可用");
    };
    let worlds = match port.list_worlds().await {
        Ok(worlds) => worlds,
        Err(error) => return map_error(error),
    };
    let Some(worlds) = worlds.into_iter().map(map_world).collect::<Option<Vec<_>>>() else {
        return internal_error();
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("业务世界状态已加载", AdminWorldListResponse { worlds })),
    )
        .into_response()
}

pub(crate) async fn set_enabled(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(world_id): Path<String>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let uri = parts.uri;
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if parse_query(&uri, &[]).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "业务世界命令不接受查询参数")
            .into_response();
    }
    if !matches!(world_id.as_str(), "main" | "isekai") {
        return api_error(StatusCode::BAD_REQUEST, "invalid_world", "业务世界无效").into_response();
    }
    let input: AdminWorldSetEnabledRequest =
        match strict_json::decode_after_guard(&headers, body, 4 << 10).await {
            Ok(input) => input,
            Err(response) => return *response,
        };
    if input.command_id.is_nil() || input.expected_policy_revision <= 0 {
        return api_error(StatusCode::BAD_REQUEST, "invalid_world_command", "业务世界命令无效")
            .into_response();
    }
    let Some(port) = state.admin_worlds() else {
        return storage_unavailable("业务世界状态暂不可用");
    };
    let result = match port
        .set_enabled(AdminWorldSetEnabledCommand {
            command_id: CommandId::from_uuid(input.command_id),
            world_id,
            enabled: input.enabled,
            expected_policy_revision: input.expected_policy_revision,
            actor_id: identity.actor,
            request_id: request_id(&headers),
        })
        .await
    {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    let Some(world) = map_world(result) else {
        return internal_error();
    };
    (
        StatusCode::OK,
        no_store_headers(),
        Json(ApiResponse::ok("业务世界状态已更新", AdminWorldSetEnabledResponse { world })),
    )
        .into_response()
}

fn map_world(world: WorldSnapshot) -> Option<AdminWorldResponse> {
    world.validate().then_some(AdminWorldResponse {
        id: world.id,
        enabled: world.enabled,
        availability: world.availability,
        policy_revision: world.policy_revision,
    })
}

fn map_error(error: ApplicationError) -> Response {
    let diagnostic_kind = DiagnosticError::diagnostic_kind(&error);
    let body = || match error {
        ApplicationError::AdminWorld(AdminWorldError::InvalidCommand) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_world_command", "业务世界命令无效")
                .into_response()
        }
        ApplicationError::AdminWorld(AdminWorldError::NotFound)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::NotFound) => {
            api_error(StatusCode::NOT_FOUND, "world_not_found", "业务世界不存在").into_response()
        }
        ApplicationError::AdminWorld(AdminWorldError::ActiveAuction) => api_error(
            StatusCode::CONFLICT,
            "world_has_active_auction",
            "该世界仍有未结束的拍卖，暂时不能关闭",
        )
        .into_response(),
        ApplicationError::AdminWorld(AdminWorldError::PolicyRevisionConflict { .. })
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::Conflict) => {
            api_error(StatusCode::CONFLICT, "stale_world_state", "业务世界状态已经变化")
                .into_response()
        }
        ApplicationError::AdminWorld(AdminWorldError::CommandConflict)
        | ApplicationError::CommandLedger(CommandLedgerError::Conflict)
        | ApplicationError::CompetitionTransaction(CompetitionTransactionError::CommandLedger(
            CommandLedgerError::Conflict,
        )) => api_error(StatusCode::CONFLICT, "command_conflict", "请求与既有命令冲突")
            .into_response(),
        ApplicationError::CapabilityUnavailable(_) => feature_unavailable(CAPABILITY_NOT_PROVIDED),
        // 存储结果损坏是确定性内部错误；503 会诱导客户端按依赖不可用重试。
        ApplicationError::AdminWorld(AdminWorldError::StoredResultCorrupt) => internal_error(),
        _ => storage_unavailable("业务世界状态暂不可用"),
    };
    let response = body();
    diagnostics::mapped_response(
        "admin_worlds",
        "respond_command_failure",
        diagnostic_kind,
        response,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::AdminWorldPort;
    use auction_application::core::ApplicationFuture;
    use axum::http::Method;
    use serde_json::json;
    use uuid::Uuid;

    use super::{AdminWorldSetEnabledCommand, StatusCode, WorldSnapshot};
    use crate::server::contract_fixtures::{admin_get, admin_request, admin_state, assert_http};

    struct Worlds;

    impl AdminWorldPort for Worlds {
        fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>> {
            Box::pin(async {
                Ok(vec![
                    WorldSnapshot {
                        id: "isekai".to_owned(),
                        enabled: false,
                        availability: "available".to_owned(),
                        policy_revision: 2,
                    },
                    WorldSnapshot {
                        id: "main".to_owned(),
                        enabled: true,
                        availability: "available".to_owned(),
                        policy_revision: 1,
                    },
                ])
            })
        }

        fn set_enabled(
            &self,
            command: AdminWorldSetEnabledCommand,
        ) -> ApplicationFuture<'_, WorldSnapshot> {
            assert_eq!(command.world_id, "isekai");
            assert!(!command.enabled);
            assert_eq!(command.expected_policy_revision, 1);
            Box::pin(async {
                Ok(WorldSnapshot {
                    id: "isekai".to_owned(),
                    enabled: false,
                    availability: "available".to_owned(),
                    policy_revision: 2,
                })
            })
        }
    }

    #[tokio::test]
    async fn world_contract_fixtures() {
        let state = admin_state().with_admin_worlds(Arc::new(Worlds));
        assert_http(
            "admin/worlds",
            StatusCode::OK,
            admin_get(state.clone(), "/api/v1/admin/worlds").await,
        )
        .await;
        let body = json!({"command_id": Uuid::from_u128(7), "enabled": false, "expected_policy_revision": 1});
        let response = admin_request(
            state,
            Method::PATCH,
            "/api/v1/admin/worlds/isekai",
            body.to_string().into(),
        )
        .await;
        assert_http("admin/world-command", StatusCode::OK, response).await;
    }
}
