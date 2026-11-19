//! Admin 凭据列表的处理器。

use std::net::SocketAddr;

use auction_application::admin::{
    AdminCredentialKind, AdminCredentialListRequest, AdminCredentialStatus, AdminCredentialWorld,
};
use auction_contracts::{
    AdminCredentialListResponse, AdminCredentialPaginationResponse, ApiResponse,
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{feature_unavailable, invalid_query, map_credential, map_error};
use crate::query::{page_offset, parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取凭据安全投影列表。
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
    let query = match parse_query(&uri, &["kind", "world", "status", "page", "page_size"]) {
        Ok(query) if query.values().all(|value| !value.is_empty()) => query,
        _ => return invalid_query(),
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let page_size = match parse_positive_i64(query.get("page_size"), DEFAULT_PAGE_SIZE, Some(200)) {
        Ok(value) => value,
        Err(_) => return invalid_query(),
    };
    let Some(offset) =
        page_offset(page, page_size, Some(auction_application::admin::MAX_ADMIN_CREDENTIAL_OFFSET))
    else {
        return invalid_query();
    };
    let kind = match query.get("kind").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialKind::parse(value) {
            Some(kind) => Some(kind),
            None => return invalid_query(),
        },
    };
    let world = match query.get("world").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialWorld::parse(value) {
            Some(world) => Some(world),
            None => return invalid_query(),
        },
    };
    let status = match query.get("status").map(String::as_str) {
        None => None,
        Some(value) => match AdminCredentialStatus::parse(value) {
            Some(status) => Some(status),
            None => return invalid_query(),
        },
    };
    let request = AdminCredentialListRequest { kind, world, status, limit: page_size, offset };
    let Some(port) = state.admin_credentials() else {
        return feature_unavailable();
    };
    let result = match port.list(request.clone()).await {
        Ok(result) => result,
        Err(error) => return map_error(error),
    };
    if !result.validate(&request) {
        return internal_error();
    }
    let response = AdminCredentialListResponse {
        credentials: result.credentials.into_iter().map(map_credential).collect(),
        pagination: AdminCredentialPaginationResponse { page, page_size, total: result.total },
        restart_required: true,
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("凭据列表已加载", response)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{
        AdminCredential, AdminCredentialDeleteCommand, AdminCredentialDeleteResult,
        AdminCredentialImportCommand, AdminCredentialKind, AdminCredentialListRequest,
        AdminCredentialPage, AdminCredentialPort, AdminCredentialRenameCommand,
        AdminCredentialStatus, AdminCredentialStatusCommand, AdminCredentialWorld,
    };
    use auction_application::core::ApplicationFuture;
    use auction_time::Duration;
    use axum::http::StatusCode;
    use uuid::Uuid;

    use crate::server::contract_fixtures::{AdminFixturePorts, admin_get, assert_http, time};

    struct FixtureCredentials;

    impl AdminCredentialPort for FixtureCredentials {
        fn list(
            &self,
            request: AdminCredentialListRequest,
        ) -> ApplicationFuture<'_, AdminCredentialPage> {
            let kind = request.kind.unwrap_or(AdminCredentialKind::MailCookie);
            let forum = kind == AdminCredentialKind::ForumCookie;
            let entry = AdminCredential {
                id: Uuid::from_u128(0x4000_8000_0000_0000_0025),
                kind,
                world: if forum {
                    AdminCredentialWorld::Shared
                } else {
                    AdminCredentialWorld::Main
                },
                status: if forum {
                    AdminCredentialStatus::Disabled
                } else {
                    AdminCredentialStatus::Active
                },
                label: "fixture-label".to_owned(),
                player_name: (!forum).then(|| "ExampleBot".to_owned()),
                state_version: if forum { 4 } else { 1 },
                created_at: time(),
                rotated_at: forum.then(|| time() + Duration::hours(1)),
                updated_at: time() + Duration::hours(2),
            };
            Box::pin(async move {
                Ok(AdminCredentialPage {
                    credentials: vec![entry],
                    total: 1,
                    limit: request.limit,
                    offset: request.offset,
                })
            })
        }

        fn import(
            &self,
            _command: AdminCredentialImportCommand,
        ) -> ApplicationFuture<'_, AdminCredential> {
            panic!("凭据列表样本不导入凭据")
        }

        fn rename(
            &self,
            command: AdminCredentialRenameCommand,
        ) -> ApplicationFuture<'_, AdminCredential> {
            Box::pin(async move {
                assert_eq!(command.credential_id, Uuid::from_u128(0x4000_8000_0000_0000_0025));
                assert_eq!(command.expected_state_version, 4);
                Ok(AdminCredential {
                    id: command.credential_id,
                    kind: AdminCredentialKind::ForumCookie,
                    world: AdminCredentialWorld::Shared,
                    status: AdminCredentialStatus::Disabled,
                    label: command.label,
                    player_name: None,
                    state_version: command.expected_state_version + 1,
                    created_at: time(),
                    rotated_at: Some(time() + Duration::hours(1)),
                    updated_at: time() + Duration::hours(3),
                })
            })
        }

        fn set_status(
            &self,
            _command: AdminCredentialStatusCommand,
        ) -> ApplicationFuture<'_, AdminCredential> {
            panic!("凭据列表样本不修改状态")
        }

        fn delete(
            &self,
            command: AdminCredentialDeleteCommand,
        ) -> ApplicationFuture<'_, AdminCredentialDeleteResult> {
            Box::pin(async move {
                Ok(AdminCredentialDeleteResult {
                    credential_id: command.credential_id,
                    state_version: command.expected_state_version + 1,
                })
            })
        }
    }

    #[tokio::test]
    async fn credentials_contract_fixtures() {
        let state = AdminFixturePorts {
            credentials: Some(Arc::new(FixtureCredentials)),
            ..Default::default()
        }
        .state();
        for (name, url) in [
            ("admin/credentials", "/api/v1/admin/credentials"),
            ("admin/credentials-forum", "/api/v1/admin/credentials?kind=forum_cookie"),
        ] {
            assert_http(name, StatusCode::OK, admin_get(state.clone(), url).await).await;
        }
    }
    #[tokio::test]
    async fn credential_command_contract_fixtures() {
        use axum::http::Method;
        use serde_json::json;

        use crate::server::contract_fixtures::admin_request;

        let state = AdminFixturePorts {
            credentials: Some(Arc::new(FixtureCredentials)),
            ..Default::default()
        }
        .state();
        let credential_id = "00000000-0000-4000-8000-000000000025";
        for (name, method, body) in [
            (
                "admin/credential-command",
                Method::PATCH,
                json!({
                    "command_id":"00000000-0000-4000-8000-000000000007",
                    "credential_id":credential_id, "expected_state_version":4, "action":"rename", "label":"New label",
                }),
            ),
            (
                "admin/credential-delete-command",
                Method::DELETE,
                json!({
                    "command_id":"00000000-0000-4000-8000-000000000008",
                    "credential_id":credential_id, "expected_state_version":5,
                }),
            ),
        ] {
            let response = admin_request(
                state.clone(),
                method,
                "/api/v1/admin/credentials",
                body.to_string().into(),
            )
            .await;
            assert_http(name, StatusCode::OK, response).await;
        }
    }
}
