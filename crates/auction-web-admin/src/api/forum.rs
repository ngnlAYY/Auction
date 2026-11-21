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
    let body = response.text().await.map_err(invalid_response_error)?;
    if !decode_sync_status(&body)? {
        return Ok(AdminForumStatus { enabled: false, thread: None });
    }

    let response = send_get(&format!("/api/v1/admin/auctions/{auction_id}/forum")).await?;
    if response.status() == 404 {
        return Ok(AdminForumStatus { enabled: true, thread: None });
    }
    if !response.ok() {
        return Err(response_error(response).await);
    }
    let body = response.text().await.map_err(invalid_response_error)?;
    let thread = decode_thread(&body, &auction_id)?;
    Ok(AdminForumStatus { enabled: true, thread: Some(thread) })
}

fn decode_sync_status(body: &str) -> Result<bool, String> {
    let payload: ApiResponse<PublicForumSyncStatusResponse> =
        serde_json::from_str(body).map_err(invalid_response_error)?;
    if !super::valid_success_code(&payload.code) {
        return Err(invalid_response_error("forum_sync_status"));
    }
    Ok(payload.data.enabled)
}

fn decode_thread(body: &str, auction_id: &str) -> Result<PublicForumResponse, String> {
    let payload: ApiResponse<PublicForumResponse> =
        serde_json::from_str(body).map_err(invalid_response_error)?;
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
                    && url.host_str() == Some("forums.e-hentai.org")
                    && url.path() == "/index.php"
                    && url.port_or_known_default() == Some(443)
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
    Ok(thread)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{decode_sync_status, decode_thread};
    use crate::api::contract_fixtures::{admin_fixture, checked};

    const AUCTION_ID: &str = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";

    #[test]
    fn forum_contract_fixtures() {
        for (body, enabled) in
            [(admin_fixture!("forum-sync"), true), (admin_fixture!("forum-sync-disabled"), false)]
        {
            assert_eq!(checked(body, "/data", "enabled", decode_sync_status), enabled);
        }
        for (body, status, published) in [
            (admin_fixture!("forum"), "published", true),
            (admin_fixture!("forum-pending"), "pending", false),
        ] {
            let thread =
                checked(body, "/data", "source_revision", |body| decode_thread(body, AUCTION_ID));
            assert_eq!(thread.auction_id, AUCTION_ID);
            assert_eq!(thread.status, status);
            assert_eq!(thread.content_synced_at.is_some(), published);
            assert_eq!(thread.thread_url.is_some(), published);
        }
    }

    #[test]
    fn forum_response_binds_identity_url_and_publication() {
        let body = admin_fixture!("forum");
        assert!(decode_thread(body, "00000000-0000-4000-8000-000000000003").is_err());
        for (key, value) in [
            ("thread_url", json!("http://forums.e-hentai.org/index.php?showtopic=123")),
            ("content_synced_at", Value::Null),
        ] {
            let mut invalid: Value = serde_json::from_str(body).unwrap();
            invalid["data"][key] = value;
            assert!(decode_thread(&invalid.to_string(), AUCTION_ID).is_err(), "{key}");
        }
    }
}
