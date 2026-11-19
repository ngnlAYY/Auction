//! Admin 用户列表的处理器。

use std::net::SocketAddr;

use auction_application::admin::{AdminUserListRequest, AdminUserStatus};
use auction_contracts::{AdminUserListResponse, AdminUserPaginationResponse, ApiResponse};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{boundary, map_list_user};
use crate::query::{page_offset, parse_positive_i64, parse_query};
use crate::session::{
    admin_session_guard, authenticated_admin_session, internal_error, no_store_headers,
};
use crate::state::AdminApiState;

const DEFAULT_PAGE_SIZE: i64 = 50;

/// 读取 Admin 用户列表。
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
    let mut query = match parse_query(&uri, &["name", "status", "page", "page_size"]) {
        Ok(query) if query.values().all(|value| !value.is_empty()) => query,
        _ => return boundary::invalid_query(),
    };
    let page = match parse_positive_i64(query.get("page"), 1, None) {
        Ok(value) => value,
        Err(_) => return boundary::invalid_query(),
    };
    let page_size = match parse_positive_i64(query.get("page_size"), DEFAULT_PAGE_SIZE, Some(200)) {
        Ok(value) => value,
        Err(_) => return boundary::invalid_query(),
    };
    let Some(offset) =
        page_offset(page, page_size, Some(auction_application::admin::MAX_ADMIN_USER_OFFSET))
    else {
        return boundary::invalid_query();
    };
    let status = match query.get("status").map(String::as_str) {
        None => None,
        Some(value) => match AdminUserStatus::parse(value) {
            Some(status) => Some(status),
            None => return boundary::invalid_query(),
        },
    };
    let request =
        AdminUserListRequest { name: query.remove("name"), status, limit: page_size, offset };
    let Some(port) = state.admin_users() else {
        return boundary::feature_unavailable();
    };
    let result = match port.list(request.clone()).await {
        Ok(result) => result,
        Err(error) => return boundary::map_list_error(error),
    };
    if !result.validate(&request) {
        return internal_error();
    }
    let response = AdminUserListResponse {
        users: result.users.into_iter().map(map_list_user).collect(),
        pagination: AdminUserPaginationResponse { page, page_size, total: result.total },
    };
    (StatusCode::OK, no_store_headers(), Json(ApiResponse::ok("用户列表已加载", response)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use auction_application::admin::{
        AdminRenameRevertCommand, AdminUser, AdminUserBlacklistHistoryEntry,
        AdminUserCreateCommand, AdminUserKeyStatus, AdminUserListItem, AdminUserListRequest,
        AdminUserMutationCommand, AdminUserMutationResult, AdminUserPage, AdminUserPort,
        AdminUserStatus,
    };
    use auction_application::core::ApplicationFuture;
    use axum::http::StatusCode;
    use uuid::Uuid;

    use crate::server::contract_fixtures::{AdminFixturePorts, admin_get, assert_http, time};

    struct FixtureUsers;

    impl AdminUserPort for FixtureUsers {
        fn list(&self, request: AdminUserListRequest) -> ApplicationFuture<'_, AdminUserPage> {
            let status = request.status.unwrap_or(AdminUserStatus::Active);
            let blacklisted = status == AdminUserStatus::Blacklisted;
            let entry = AdminUserListItem {
                user: AdminUser {
                    id: Uuid::from_u128(0x4000_8000_0000_0000_0024),
                    username: "ExamplePlayer".to_owned(),
                    forum_account_id: blacklisted.then(|| "12345".to_owned()),
                    status,
                    key_status: if blacklisted {
                        AdminUserKeyStatus::Revoked
                    } else {
                        AdminUserKeyStatus::Active
                    },
                    state_version: if blacklisted { 3 } else { 1 },
                    created_at: time(),
                    updated_at: time(),
                },
                blacklist_count: i64::from(blacklisted),
                blacklist_history: blacklisted
                    .then(|| AdminUserBlacklistHistoryEntry {
                        public_reason: "示例原因".to_owned(),
                        blacklisted_at: time(),
                    })
                    .into_iter()
                    .collect(),
            };
            Box::pin(async move {
                Ok(AdminUserPage {
                    users: vec![entry],
                    total: 1,
                    limit: request.limit,
                    offset: request.offset,
                })
            })
        }

        fn create(
            &self,
            command: AdminUserCreateCommand,
        ) -> ApplicationFuture<'_, AdminUserMutationResult> {
            use auction_application::admin::AdminUserKeyMode;
            use auction_application::mail::MailLoginKey;

            Box::pin(async move {
                assert_eq!(command.key_mode, AdminUserKeyMode::Generated);
                Ok(AdminUserMutationResult {
                    user: AdminUser {
                        id: Uuid::from_u128(0x4000_8000_0000_0000_0026),
                        username: command.username,
                        forum_account_id: command.forum_account_id,
                        status: AdminUserStatus::Active,
                        key_status: AdminUserKeyStatus::Active,
                        state_version: 1,
                        created_at: time(),
                        updated_at: time(),
                    },
                    // 这是固定的占位数据，未生成或写入真实登录 key。
                    generated_key: Some(
                        MailLoginKey::from_encoded("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
                            .unwrap(),
                    ),
                })
            })
        }

        fn mutate(
            &self,
            _command: AdminUserMutationCommand,
        ) -> ApplicationFuture<'_, AdminUserMutationResult> {
            panic!("用户列表样本不修改用户")
        }

        fn revert_rename(
            &self,
            command: AdminRenameRevertCommand,
        ) -> ApplicationFuture<'_, AdminUserMutationResult> {
            Box::pin(async move {
                assert_eq!(command.rename_id, Uuid::from_u128(0x4000_8000_0000_0000_0008));
                assert_eq!(command.expected_state_version, 3);
                Ok(AdminUserMutationResult {
                    user: AdminUser {
                        id: Uuid::from_u128(0x4000_8000_0000_0000_0009),
                        username: "Previous".to_owned(),
                        forum_account_id: None,
                        status: AdminUserStatus::Active,
                        key_status: AdminUserKeyStatus::Active,
                        state_version: command.expected_state_version + 1,
                        created_at: time(),
                        updated_at: time() + auction_time::Duration::minutes(1),
                    },
                    generated_key: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn user_list_contract_fixtures() {
        let state =
            AdminFixturePorts { users: Some(Arc::new(FixtureUsers)), ..Default::default() }.state();
        for (name, url) in [
            ("admin/users", "/api/v1/admin/users"),
            ("admin/users-blacklisted", "/api/v1/admin/users?status=blacklisted"),
        ] {
            assert_http(name, StatusCode::OK, admin_get(state.clone(), url).await).await;
        }
    }
    #[tokio::test]
    async fn rename_revert_contract_fixtures() {
        use axum::http::Method;
        use serde_json::json;

        use crate::server::contract_fixtures::admin_request;

        let state =
            AdminFixturePorts { users: Some(Arc::new(FixtureUsers)), ..Default::default() }.state();
        let body =
            json!({"command_id":"00000000-0000-4000-8000-000000000007","expected_state_version":3});
        let response = admin_request(
            state.clone(),
            Method::POST,
            "/api/v1/admin/renames/00000000-0000-4000-8000-000000000008/revert",
            body.to_string().into(),
        )
        .await;
        assert_http("admin/rename-revert-command", StatusCode::OK, response).await;

        let body = json!({"command_id":"00000000-0000-4000-8000-000000000008","username":"ExamplePlayer","key_mode":"generated"});
        let response =
            admin_request(state, Method::POST, "/api/v1/admin/users", body.to_string().into())
                .await;
        assert_http("admin/user-create-command", StatusCode::CREATED, response).await;
    }
}
