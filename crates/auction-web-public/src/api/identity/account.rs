//! Public 账户读取 DTO 与同源请求。
//! 这里只解码后端的安全账户 projection；代理上限、历史转换和 capability 仍以服务端返回的
//! 公开字段为准，浏览器不重建代理状态机。

pub(crate) use auction_contracts::{
    PublicMyBidsResponse as MyBidsPage, PublicOwnedBidResponse as OwnedBid,
    PublicOwnedProxyResponse as OwnedProxy,
};
use gloo_timers::future::TimeoutFuture;

use super::{decoder, send_get};

const PUBLIC_QUERY_RETRY_DELAY_MILLIS: u32 = 1_000;

pub(crate) async fn fetch_my_bids(bid_page: i64, proxy_page: i64) -> Result<MyBidsPage, String> {
    let url = format!("/api/v1/me/bids?bid_page={bid_page}&page_size=10&proxy_page={proxy_page}");
    if let Ok(page) = fetch_my_bids_once(&url).await {
        return Ok(page);
    }
    TimeoutFuture::new(PUBLIC_QUERY_RETRY_DELAY_MILLIS).await;
    fetch_my_bids_once(&url).await
}

async fn fetch_my_bids_once(url: &str) -> Result<MyBidsPage, String> {
    let response = send_get(url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_my_bids(response).await
}

pub(crate) async fn fetch_my_proxy(lot_id: String) -> Result<Option<OwnedProxy>, String> {
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations/mine");
    if let Ok(proxy) = fetch_my_proxy_once(&url, &lot_id).await {
        return Ok(proxy);
    }
    TimeoutFuture::new(PUBLIC_QUERY_RETRY_DELAY_MILLIS).await;
    fetch_my_proxy_once(&url, &lot_id).await
}

async fn fetch_my_proxy_once(url: &str, lot_id: &str) -> Result<Option<OwnedProxy>, String> {
    let response = send_get(url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    Ok(decoder::decode_my_proxy(response, lot_id).await?.authorization)
}
