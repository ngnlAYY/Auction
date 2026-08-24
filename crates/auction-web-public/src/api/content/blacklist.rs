//! Public 黑名单 HTTP DTO 与请求。

pub(crate) use auction_contracts::{
    PublicBlacklistEntryResponse as BlacklistEntry, PublicBlacklistResponse as BlacklistPage,
};

use super::{ApiResponse, send_get};

pub(crate) async fn fetch_blacklist(page: i64) -> Result<BlacklistPage, String> {
    let page = page.max(1);
    let response = send_get(&format!("/api/v1/blacklist?page={page}&page_size=10")).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<BlacklistPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
