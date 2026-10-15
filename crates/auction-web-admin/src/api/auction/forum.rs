//! Admin 论坛状态只读请求；与 Public 共用 contracts DTO，不依赖 Public Web。

use auction_contracts::{ApiResponse, PublicForumResponse, PublicForumSyncStatusResponse};

use super::{invalid_response_error, response_error, send_get, valid_admin_time, valid_uuid};

/// 同步启用状态与当前拍卖的安全投影；没有镜像不等于同步失败。
#[derive(Clone, PartialEq)]
pub(crate) struct AdminForumStatus {
    pub enabled: bool,
    pub thread: Option<PublicForumResponse>,
}

/// 只重读本地状态，不创建恢复命令，不触发远端 POST。
pub(crate) async fn fetch_admin_forum_status(
    auction_id: String,
) -> Result<AdminForumStatus, String> {
    if !valid_uuid(&auction_id) {
        return Err("invalid_request".to_owned());
    }
    let response = send_get("/api/v1/admin/forum/sync-status").await?;
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<PublicForumSyncStatusResponse> =
        response.json().await.map_err(invalid_response_error)?;
    if !super::valid_success_code(&payload.code) {
        return Err(invalid_response_error("forum_sync_status"));
    }
    if !payload.data.enabled {
        return Ok(AdminForumStatus { enabled: false, thread: None });
    }

    let response = send_get(&format!("/api/v1/admin/auctions/{auction_id}/forum")).await?;
    if response.status() == 404 {
        return Ok(AdminForumStatus { enabled: true, thread: None });
    }
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let payload: ApiResponse<PublicForumResponse> =
        response.json().await.map_err(invalid_response_error)?;
    let thread = payload.data;
    if !super::valid_success_code(&payload.code)
        || thread.auction_id != auction_id
        || !matches!(thread.status.as_str(), "pending" | "publishing" | "published" | "failed")
        || thread.source_revision < 0
        || thread.content_revision < 0
        || thread.processed_through_post_number < 2
        || !valid_admin_time(&thread.updated_at)
        || ![
            thread.content_synced_at.as_deref(),
            thread.content_last_success_at.as_deref(),
            thread.content_last_error_at.as_deref(),
        ]
        .into_iter()
        .all(|value| value.is_none_or(valid_admin_time))
        || thread.content_last_error_at.is_some() != thread.content_last_error_code.is_some()
        || thread.content_last_error_code.as_deref().is_some_and(|code| {
            code.is_empty()
                || code.len() > 64
                || !code.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        })
        || thread.thread_url.as_deref().is_some_and(|value| {
            !url::Url::parse(value).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
        })
        || thread.status == "published"
            && (thread.content_revision == 0
                || thread.content_synced_at.is_none()
                || thread.thread_url.is_none()
                || thread.title.as_deref().is_none_or(str::is_empty)
                || thread.body.as_deref().is_none_or(str::is_empty))
    {
        return Err(invalid_response_error("forum_thread"));
    }
    Ok(AdminForumStatus { enabled: true, thread: Some(thread) })
}
