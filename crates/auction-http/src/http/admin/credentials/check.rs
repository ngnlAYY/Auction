//! 已保存 Cookie 的手动认证检查；保留 Admin 会话、Origin、CSRF、限流和版本门禁。

use std::net::SocketAddr;

use auction_contracts::{
    AdminCredentialCheckRequest, AdminCredentialCheckResponse, AdminCredentialCheckScopeResponse,
    ApiResponse,
};
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use super::{
    AdminApiState, authenticated_admin_mutation, feature_unavailable, invalid_json, map_error,
    mutation_boundary, no_store_headers, timestamp,
};

pub(crate) async fn check(
    State(state): State<AdminApiState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    Path(credential_id): Path<Uuid>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if let Err(response) = authenticated_admin_mutation(&state, &headers, address).await {
        return *response;
    }
    if let Some(response) = mutation_boundary(&uri) {
        return response;
    }
    let input: AdminCredentialCheckRequest = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return invalid_json(),
    };
    let Some(port) = state.credential_check() else { return feature_unavailable() };
    match port.check(credential_id, input.expected_state_version).await {
        Ok(result) => (
            StatusCode::OK,
            no_store_headers(),
            Json(ApiResponse::ok(
                "Cookie 校验完成",
                AdminCredentialCheckResponse {
                    credential_id: result.credential_id.to_string(),
                    state_version: result.state_version,
                    checked_at: timestamp(result.checked_at),
                    checks: result
                        .checks
                        .into_iter()
                        .map(|check| AdminCredentialCheckScopeResponse {
                            world: check.world.as_str().to_owned(),
                            status: check.status.as_str().to_owned(),
                        })
                        .collect(),
                },
            )),
        )
            .into_response(),
        Err(error) => map_error(error),
    }
}
