//! Public 黑名单 HTTP DTO 与请求。

pub(crate) use auction_contracts::{
    PublicBlacklistEntryResponse as BlacklistEntry, PublicBlacklistResponse as BlacklistPage,
};

use super::{decoder, send_get};

pub(crate) async fn fetch_blacklist(page: i64, page_size: i64) -> Result<BlacklistPage, String> {
    let page = page.max(1);
    let response = send_get(&format!("/api/v1/blacklist?page={page}&page_size={page_size}"))
        .await
        .map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(decoder::decode_error_code(response).await);
    }
    decoder::decode_blacklist(response).await
}
