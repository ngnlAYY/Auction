//! Public 论坛镜像 HTTP DTO 与请求。

use chrono::DateTime;
use url::Url;

pub(crate) use auction_contracts::PublicForumResponse as ForumThread;
use auction_contracts::PublicForumSyncStatusResponse;
use auction_web_shared::{BrowserErrorClass, report_browser_error};

use super::{ApiResponse, send_get};

/// 开关只来自 Public 的安全投影；请求或解码失败不能当作关闭。
pub(crate) async fn fetch_forum_sync_enabled() -> Result<bool, String> {
    let response =
        send_get("/api/v1/forum/sync-status").await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<PublicForumSyncStatusResponse> =
        response.json().await.map_err(|error| {
            report_browser_error(BrowserErrorClass::DtoDecode);
            error.to_string()
        })?;
    Ok(payload.data.enabled)
}

pub(crate) async fn fetch_forum_thread(auction_id: String) -> Result<ForumThread, String> {
    let url = format!("/api/v1/auctions/{auction_id}/forum");
    let response = send_get(&url).await.map_err(|error| error.code().to_owned())?;
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let payload: ApiResponse<ForumThread> = response.json().await.map_err(|error| {
        report_browser_error(BrowserErrorClass::DtoDecode);
        error.to_string()
    })?;
    if !valid_forum_thread(&payload.data, &auction_id) {
        report_browser_error(BrowserErrorClass::DtoDecode);
        return Err("invalid_response".to_owned());
    }
    Ok(payload.data)
}

/// 恢复旧浏览器的严格语义边界，避免形状正确但内容损坏的论坛投影进入页面。
fn valid_forum_thread(thread: &ForumThread, auction_id: &str) -> bool {
    thread.auction_id == auction_id
        && auction_web_shared::is_legacy_uuid(&thread.auction_id)
        && thread.source_revision >= 0
        && thread.content_revision >= 0
        && thread.processed_through_post_number >= 2
        && DateTime::parse_from_rfc3339(&thread.updated_at).is_ok()
        && valid_optional_timestamp(thread.content_synced_at.as_deref())
        && valid_optional_timestamp(thread.content_last_success_at.as_deref())
        && valid_optional_timestamp(thread.content_last_error_at.as_deref())
        && thread.content_last_error_at.is_some() == thread.content_last_error_code.is_some()
        && thread.content_last_error_code.as_deref().is_none_or(valid_error_code)
        && matches!(thread.status.as_str(), "failed" | "pending" | "published" | "publishing")
        && (thread.status != "published"
            || (thread.content_revision > 0
                && thread.content_synced_at.is_some()
                && thread.thread_url.as_deref().is_some_and(valid_forum_url)
                && [&thread.title, &thread.body]
                    .into_iter()
                    .all(|value| value.as_ref().is_some_and(|value| !value.is_empty()))))
}

fn valid_optional_timestamp(value: Option<&str>) -> bool {
    value.is_none_or(|value| DateTime::parse_from_rfc3339(value).is_ok())
}

fn valid_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value == value.trim()
        && value.chars().all(|character| character.is_ascii_lowercase() || character == '_')
}

/// 外部链接只接受无用户信息的 HTTPS URL，避免损坏投影把危险 scheme 注入浏览器。
pub(super) fn valid_forum_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}
