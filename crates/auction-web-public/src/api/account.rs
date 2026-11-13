//! Public 账户读取 DTO 与同源请求。
//! 这里只解码后端的安全账户 projection；代理上限、历史转换和 capability 仍以服务端返回的
//! 公开字段为准，浏览器不重建代理状态机。

pub(crate) use auction_contracts::{
    PublicMyBidsResponse as MyBidsPage, PublicOwnedBidResponse as OwnedBid,
    PublicOwnedProxyResponse as OwnedProxy,
};
use auction_web_shared::timer::TimeoutFuture;
use gloo_net::http::{Request, Response};

use super::{TransportError, decoder, send_request};

const PUBLIC_QUERY_RETRY_DELAY_MILLIS: u32 = 1_000;

pub(crate) async fn fetch_my_bids(
    user_id: &str,
    bid_page: i64,
    proxy_page: i64,
) -> Result<MyBidsPage, String> {
    let url = format!("/api/v1/me/bids?bid_page={bid_page}&page_size=10&proxy_page={proxy_page}");
    if let Ok(page) = fetch_my_bids_once(&url, user_id).await {
        return Ok(page);
    }
    TimeoutFuture::new(PUBLIC_QUERY_RETRY_DELAY_MILLIS).await;
    fetch_my_bids_once(&url, user_id).await
}

async fn fetch_my_bids_once(url: &str, user_id: &str) -> Result<MyBidsPage, String> {
    let response = account_response(url, user_id).await?;
    decoder::decode_my_bids(response).await
}

pub(crate) async fn fetch_my_proxy(
    user_id: &str,
    lot_id: String,
) -> Result<Option<OwnedProxy>, String> {
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations/mine");
    if let Ok(proxy) = fetch_my_proxy_once(&url, user_id, &lot_id).await {
        return Ok(proxy);
    }
    TimeoutFuture::new(PUBLIC_QUERY_RETRY_DELAY_MILLIS).await;
    fetch_my_proxy_once(&url, user_id, &lot_id).await
}

async fn fetch_my_proxy_once(
    url: &str,
    user_id: &str,
    lot_id: &str,
) -> Result<Option<OwnedProxy>, String> {
    let response = account_response(url, user_id).await?;
    Ok(decoder::decode_my_proxy(response, lot_id).await?.authorization)
}

/// 响应不携带用户 UUID，必须由同一请求的 Cookie 与预期宿主身份校验绑定归属。
async fn account_response(url: &str, user_id: &str) -> Result<Response, String> {
    let response = send_request(|abort_signal| {
        Request::get(url)
            .abort_signal(abort_signal)
            .header(auction_contracts::PUBLIC_EXPECTED_USER_ID_HEADER, user_id)
            .build()
            .map_err(|_| TransportError::Network(None))
    })
    .await
    .map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        let status = response.status();
        let code = decoder::decode_error_code(response).await;
        super::session::observe_public_session_failure(status, &code);
        return Err(code);
    }
    Ok(response)
}
