//! Public 论坛镜像 HTTP DTO 与请求。

use gloo_net::http::Request;
use serde::Deserialize;

use super::ApiResponse;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub(crate) struct ForumThread {
    pub auction_id: String,
    pub status: String,
    pub source_revision: i64,
    pub thread_url: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub updated_at: String,
}

pub(crate) async fn fetch_forum_thread(auction_id: String) -> Result<ForumThread, String> {
    let url = format!("/api/v1/auctions/{auction_id}/forum");
    let response = Request::get(&url).send().await.map_err(|error| error.to_string())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<ForumThread> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
