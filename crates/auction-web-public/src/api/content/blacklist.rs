//! Public 黑名单 HTTP DTO 与请求。

pub(crate) use auction_contracts::{
    PublicBlacklistEntryResponse as BlacklistEntry, PublicBlacklistResponse as BlacklistPage,
};

use super::{decoder, send_get};

pub(crate) async fn fetch_blacklist(page: i64) -> Result<BlacklistPage, String> {
    let page = page.max(1);
    let response = send_get(&format!("/api/v1/blacklist?page={page}&page_size=10")).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    decoder::decode_blacklist(response).await
}
