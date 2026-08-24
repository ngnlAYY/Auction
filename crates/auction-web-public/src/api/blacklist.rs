//! Public 黑名单 HTTP DTO 与请求。

use serde::Deserialize;

use super::{ApiResponse, send_get};

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct BlacklistEntry {
    pub game_player_id: String,
    pub reason: String,
    pub blacklisted_at: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct BlacklistPage {
    pub users: Vec<BlacklistEntry>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

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
