//! Public 论坛镜像 DTO。

use super::*;

/// Public 论坛镜像同步状态响应。
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
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
