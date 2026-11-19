//! 会话当前状态和管理端状态名称的响应投影。

use auction_application::public::PublicSessionIdentity;
use auction_contracts::{ApiResponse, PublicSessionCurrentResponse};
use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::mapping::public_timestamp;

/// 只投影权威身份，不修改浏览器 Cookie；迟到的匿名或旧身份读取不能覆盖新登录。
pub(crate) fn current_session_response(identity: Option<PublicSessionIdentity>) -> Response {
    let response_headers = no_store_headers();
    let data = identity.map(|identity| PublicSessionCurrentResponse {
        authenticated: true,
        user_id: identity.user_id.to_string(),
        username: identity.username,
        expires_at: identity.expires_at.map(public_timestamp),
        absolute_expires_at: identity.absolute_expires_at.map(public_timestamp),
    });
    let message = if data.is_some() { "会话有效" } else { "当前未登录" };
    (StatusCode::OK, response_headers, Json(ApiResponse::ok(message, data))).into_response()
}

pub(crate) fn admin_auction_status_name(status: auction_domain::AuctionStatus) -> &'static str {
    match status {
        auction_domain::AuctionStatus::Draft => "draft",
        auction_domain::AuctionStatus::Preview => "preview",
        auction_domain::AuctionStatus::Live => "live",
        auction_domain::AuctionStatus::Final => "final",
        auction_domain::AuctionStatus::Settling => "settling",
        auction_domain::AuctionStatus::Settled => "settled",
        auction_domain::AuctionStatus::Cancelled => "cancelled",
    }
}

pub(crate) fn no_store_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers
}

#[cfg(test)]
mod tests {
    use auction_application::public::PublicSessionIdentity;
    use auction_time::{Duration, Timestamp};
    use axum::http::StatusCode;
    use uuid::Uuid;

    use super::current_session_response;
    use crate::server::contract_fixtures::assert_http;

    #[tokio::test]
    async fn timed_session_contract_fixture() {
        let expires_at = Timestamp::from_timestamp(1_763_200_000, 0).expect("截止时刻");
        let response = current_session_response(Some(PublicSessionIdentity {
            user_id: Uuid::from_u128(0x0190_f2b4_1c2d_4e5f_8a6b_7c8d_9e0f_1a2b),
            username: "ExamplePlayer".to_owned(),
            expires_at: Some(expires_at),
            absolute_expires_at: Some(expires_at + Duration::hours(1)),
        }));
        assert_http("public/session", StatusCode::OK, response).await;
    }
}
