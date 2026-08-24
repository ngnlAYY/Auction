//! Public 论坛镜像 HTTP DTO 与请求。

pub(crate) use auction_contracts::PublicForumResponse as ForumThread;

use super::{ApiResponse, send_get};

pub(crate) async fn fetch_forum_thread(auction_id: String) -> Result<ForumThread, String> {
    let url = format!("/api/v1/auctions/{auction_id}/forum");
    let response = send_get(&url).await?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<ForumThread> =
        response.json().await.map_err(|error| error.to_string())?;
    Ok(payload.data)
}
