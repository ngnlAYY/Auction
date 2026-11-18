//! Admin 创建拍卖的处理器。

use std::net::SocketAddr;

use auction_application::admin::AdminCreateAuctionCommand;
use auction_application::core::RequestHash;
use auction_application::core::diagnostics::{DiagnosticContext, DiagnosticError};
use auction_contracts::{AdminAuctionCreateResponse, AdminCreateAuctionRequest, ApiResponse};
use auction_domain::{AuctionId, CommandId};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{feature_unavailable, invalid_create_command, map_create_error};
use crate::diagnostics;
use crate::errors::api_error;
use crate::middleware::boundary::request_id;
use crate::query::parse_canonical_uuid;
use crate::session::{authenticated_admin_mutation, internal_error, no_store_headers};
use crate::state::AdminApiState;

/// 创建 Admin draft 拍卖。
pub(crate) async fn create(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    let (_, identity) = match authenticated_admin_mutation(&state, &headers, address).await {
        Ok(result) => result,
        Err(response) => return *response,
    };
    if uri.query().is_some_and(|query| !query.is_empty()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_query", "创建拍卖不接受查询参数")
            .into_response();
    }
    let input: AdminCreateAuctionRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_json", "请求 JSON 无效")
                .into_response();
        }
    };
    let Some(command_id) = parse_canonical_uuid(&input.command_id) else {
        return invalid_create_command();
    };
    let Some(auction_id) = parse_canonical_uuid(&input.auction_id) else {
        return invalid_create_command();
    };
    let (Some(preview_scheduled_at), Some(scheduled_open_at), Some(scheduled_end_at)) =
        (input.preview_scheduled_at, input.scheduled_open_at, input.scheduled_end_at)
    else {
        return invalid_create_command();
    };
    let request_hash = match RequestHash::from_request("admin_auction_create", auction_id, &input) {
        Ok(hash) => hash,
        Err(diagnostic_cause) => {
            diagnostics::error(
                "admin_auctions",
                "hash_create",
                DiagnosticError::diagnostic_kind(&diagnostic_cause),
                DiagnosticContext::default(),
            );
            return internal_error();
        }
    };
    let command = AdminCreateAuctionCommand {
        command_id: CommandId::from_uuid(command_id),
        auction_id: AuctionId::from_uuid(auction_id),
        world_id: input.world_id.clone(),
        sequence: input.sequence,
        preview_scheduled_at,
        scheduled_open_at,
        scheduled_end_at,
        expected_state_version: input.expected_state_version,
        actor_id: identity.actor,
        request_id: request_id(&headers),
        request_hash,
    };
    if !command.validate() {
        return invalid_create_command();
    }
    let Some(port) = state.auction_create() else {
        return feature_unavailable();
    };
    let result = match port.create(command).await {
        Ok(result) => result,
        Err(error) => return map_create_error(error),
    };
    if !result.validate()
        || result.auction.id.as_uuid() != auction_id
        || result.auction.world_id != input.world_id
        || result.auction.sequence != input.sequence
        || result.auction.status != "draft"
        || result.auction.state_version != 1
    {
        return internal_error();
    }
    (
        StatusCode::CREATED,
        no_store_headers(),
        Json(ApiResponse::created(
            "后台拍卖已创建",
            AdminAuctionCreateResponse { auction: super::map_auction_command(result) },
        )),
    )
        .into_response()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use auction_application::admin::AdminRuntimeConfigPort;
    use auction_application::core::ApplicationFuture;
    use auction_application::public::{AdminLoginResult, AdminSessionIdentity, AdminSessionPort};
    use auction_contracts::ApiErrorResponse;
    use auction_time::Timestamp;
    use axum::body::{Bytes, to_bytes};
    use axum::extract::{ConnectInfo, State};
    use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
    use axum::response::Response;
    use serde_json::json;

    use super::create;
    use crate::AdminApiState;
    use crate::admin::{ADMIN_CSRF_COOKIE, ADMIN_CSRF_HEADER, ADMIN_SESSION_COOKIE};

    pub(crate) const VALID_UUID: &str = "5b1c2a9e-1f0d-4c6b-9a7e-3d2f1e0c9b8a";
    const V7_UUID: &str = "01976b9c-af94-7b04-89fa-5b49708c9faa";

    struct Session;

    impl AdminSessionPort for Session {
        fn login(
            &self,
            _actor: String,
            _secret: Vec<u8>,
            _now: Timestamp,
        ) -> ApplicationFuture<'_, AdminLoginResult> {
            unreachable!("测试只使用已有会话");
        }

        fn current(
            &self,
            _token: String,
            _csrf: String,
            now: Timestamp,
        ) -> ApplicationFuture<'_, AdminSessionIdentity> {
            Box::pin(async move {
                Ok(AdminSessionIdentity {
                    actor: "operator".to_owned(),
                    expires_at: now,
                    absolute_expires_at: now,
                })
            })
        }

        fn logout(&self, _token: String, _now: Timestamp) -> ApplicationFuture<'_, ()> {
            unreachable!("测试只使用已有会话");
        }
    }

    pub(crate) fn state() -> State<AdminApiState> {
        state_with_runtime(None)
    }

    pub(crate) fn state_with_runtime(
        runtime: Option<Arc<dyn AdminRuntimeConfigPort>>,
    ) -> State<AdminApiState> {
        // 提供已认证会话和测试指定的端口，其他业务能力保持未装配。
        State(AdminApiState::new(
            Some(Arc::new(Session)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            runtime,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Default::default(),
        ))
    }

    pub(crate) fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!(
                "{ADMIN_SESSION_COOKIE}=fixture; {ADMIN_CSRF_COOKIE}=fixture"
            ))
            .unwrap(),
        );
        headers.insert(ADMIN_CSRF_HEADER, HeaderValue::from_static("fixture"));
        headers
    }

    pub(crate) async fn assert_code(response: Response, status: StatusCode, code: &str) {
        assert_eq!(response.status(), status);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: ApiErrorResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.code, code);
    }

    #[tokio::test]
    async fn admin_create_auction_http_admission_matches_public_uuid_and_sequence() {
        let address = ConnectInfo("127.0.0.1:1234".parse().unwrap());
        for (auction_id, sequence, valid) in [
            (VALID_UUID, 9_007_199_254_740_991_i64, true),
            (VALID_UUID, 9_007_199_254_740_992, false),
            (V7_UUID, 1, false),
            ("5B1C2A9E-1F0D-4C6B-9A7E-3D2F1E0C9B8A", 1, false),
            ("5b1c2a9e-1f0d-4c6b-0a7e-3d2f1e0c9b8a", 1, false),
        ] {
            let body = json!({
                "command_id": V7_UUID,
                "auction_id": auction_id,
                "sequence": sequence,
                "world_id": "main",
                "preview_scheduled_at": "2026-12-01T00:00:00Z",
                "scheduled_open_at": "2026-12-01T01:00:00Z",
                "scheduled_end_at": "2026-12-01T02:00:00Z",
                "expected_state_version": 0,
            });
            let response =
                create(state(), address, headers(), Uri::default(), Bytes::from(body.to_string()))
                    .await;
            if valid {
                assert_code(response, StatusCode::NOT_IMPLEMENTED, "feature_not_supported").await;
            } else {
                assert_code(response, StatusCode::BAD_REQUEST, "invalid_auction_command").await;
            }
        }
    }
}
