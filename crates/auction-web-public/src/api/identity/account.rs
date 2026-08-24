//! Public 账户读取 DTO 与同源请求。
//!
//! 这里只解码后端的安全账户 projection；代理上限、历史转换和 capability 仍以服务端返回的
//! 公开字段为准，浏览器不重建代理状态机。

use auction_contracts::{ApiResponse, PublicMyProxyResponse};
pub(crate) use auction_contracts::{
    PublicMyBidsResponse as MyBidsPage, PublicOwnedBidResponse as OwnedBid,
    PublicOwnedProxyResponse as OwnedProxy,
};

use super::send_get;

pub(crate) async fn fetch_my_bids(bid_page: i64, proxy_page: i64) -> Result<MyBidsPage, String> {
    let url = format!("/api/v1/me/bids?bid_page={bid_page}&proxy_page={proxy_page}&page_size=20");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<MyBidsPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}

pub(crate) async fn fetch_my_proxy(lot_id: String) -> Result<Option<OwnedProxy>, String> {
    let url = format!("/api/v1/lots/{lot_id}/proxy-authorizations/mine");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<PublicMyProxyResponse> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data.authorization)
}
