//! Public 黑名单 HTTP DTO 与请求。

use gloo_net::http::Request;
use serde::Deserialize;

use super::ApiResponse;

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

pub(crate) async fn fetch_blacklist() -> Result<BlacklistPage, String> {
    let response = Request::get("/api/v1/blacklist?page=1&page_size=50")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<BlacklistPage> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
