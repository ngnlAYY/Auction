//! Public 论坛镜像读取端口。
//!
//! 该端口只读取本地已经持久化的论坛同步投影，不直接调用论坛网关，也不把远程发布成功
//! 推断为本地状态。未接入的后端必须显式报告能力缺失。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::ApplicationFuture;

/// Public 论坛同步状态的闭集。
pub const PUBLIC_FORUM_STATUSES: [&str; 4] = ["pending", "publishing", "published", "failed"];

/// Public 论坛镜像安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicForumThread {
    pub auction_id: Uuid,
    pub status: String,
    pub source_revision: i64,
    pub thread_url: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl PublicForumThread {
    /// 校验同步状态及已发布内容的完整性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || !PUBLIC_FORUM_STATUSES.contains(&self.status.as_str())
            || self.status != self.status.trim()
            || self.status.chars().any(char::is_control)
            || self.source_revision < 0
            || self.updated_at.timestamp_micros() == 0
            || !valid_optional_text(self.thread_url.as_deref(), 2048)
            || !valid_optional_text(self.title.as_deref(), 512)
            || !valid_optional_multiline_text(self.body.as_deref(), 1_000_000)
        {
            return Err("公开论坛同步投影无效");
        }
        if self.status == "published"
            && [self.thread_url.as_deref(), self.title.as_deref(), self.body.as_deref()]
                .into_iter()
                .any(|value| value.is_none_or(str::is_empty))
        {
            return Err("已发布论坛投影缺少内容");
        }
        Ok(())
    }
}

fn valid_optional_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| {
        !value.is_empty()
            && value == value.trim()
            && value.len() <= maximum
            && !value.chars().any(char::is_control)
    })
}

fn valid_optional_multiline_text(value: Option<&str>, maximum: usize) -> bool {
    value.is_none_or(|value| {
        !value.trim().is_empty()
            && value.len() <= maximum
            && !value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    })
}

/// Public 论坛镜像读取端口。
pub trait PublicForumReader: Send + Sync {
    fn public_forum_thread(
        &self,
        auction_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicForumThread>>;
}
