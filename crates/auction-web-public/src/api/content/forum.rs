//! Public 论坛镜像 HTTP DTO 与请求。

use chrono::DateTime;
use url::Url;
use uuid::{Uuid, Variant};

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
    if !valid_forum_thread(&payload.data, &auction_id) {
        return Err("invalid_response".to_owned());
    }
    Ok(payload.data)
}

/// 恢复旧浏览器的严格语义边界，避免形状正确但内容损坏的论坛投影进入页面。
fn valid_forum_thread(thread: &ForumThread, auction_id: &str) -> bool {
    thread.auction_id == auction_id
        && valid_legacy_uuid(&thread.auction_id)
        && thread.source_revision >= 0
        && DateTime::parse_from_rfc3339(&thread.updated_at).is_ok()
        && matches!(thread.status.as_str(), "failed" | "pending" | "published" | "publishing")
        && (thread.status != "published"
            || (thread.thread_url.as_deref().is_some_and(valid_forum_url)
                && [&thread.title, &thread.body]
                    .into_iter()
                    .all(|value| value.as_ref().is_some_and(|value| !value.is_empty()))))
}

/// 外部链接只接受无用户信息的 HTTPS URL，避免损坏投影把危险 scheme 注入浏览器。
fn valid_forum_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

/// 旧 Web 只接受小写 canonical、RFC 4122 variant 且版本为 1 至 5 的 UUID。
fn valid_legacy_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| {
        id.hyphenated().to_string() == value
            && matches!(id.get_version_num(), 1..=5)
            && id.get_variant() == Variant::RFC4122
    })
}
