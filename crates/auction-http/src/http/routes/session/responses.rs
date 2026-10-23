//! 会话当前状态和管理端状态名称的响应投影。

use super::cookies::append_clear_cookie;
use super::*;

pub(crate) fn current_session_response(
    state: &PublicApiState,
    identity: Option<PublicSessionIdentity>,
    clear_cookie: bool,
) -> Response {
    let mut response_headers = no_store_headers();
    if clear_cookie {
        append_clear_cookie(&mut response_headers, state.security().cookie_secure());
    }
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
