//! Public 论坛镜像 DTO。

use super::*;

/// Public 论坛同步启用状态；未知状态通过错误响应表达，不伪造默认布尔值。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicForumSyncStatusResponse {
    pub enabled: bool,
}

/// 论坛镜像正文可能包含完整装备 URL，调试输出与发送前正文保持同一脱敏边界。
#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicForumResponse {
    pub auction_id: String,
    pub status: String,
    pub source_revision: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub content_revision: i64,
    pub processed_through_post_number: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_synced_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_last_success_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_last_error_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_last_error_code: Option<String>,
    pub updated_at: String,
}

impl std::fmt::Debug for PublicForumResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicForumResponse")
            .field("auction_id", &self.auction_id)
            .field("status", &self.status)
            .field("source_revision", &self.source_revision)
            .field("thread_url", &self.thread_url)
            .field("title", &self.title)
            .field("body", &self.body.as_ref().map(|_| "[REDACTED]"))
            .field("content_revision", &self.content_revision)
            .field("processed_through_post_number", &self.processed_through_post_number)
            .field("content_synced_at", &self.content_synced_at)
            .field("content_last_success_at", &self.content_last_success_at)
            .field("content_last_error_at", &self.content_last_error_at)
            .field("content_last_error_code", &self.content_last_error_code)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}
