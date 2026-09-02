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
    /// 固定 #1/#2 的本地 CAS 版本；不暴露 Worker claim 或下次调度时间。
    pub content_revision: i64,
    /// 最近一次取得远端固定楼层正文证明的时间。
    pub content_synced_at: Option<DateTime<Utc>>,
    /// 最近一次完整扫描成功的时间。
    pub content_last_success_at: Option<DateTime<Utc>>,
    /// 当前未恢复扫描错误的发生时间。
    pub content_last_error_at: Option<DateTime<Utc>>,
    /// 当前未恢复扫描错误的有界稳定码，不包含远端正文。
    pub content_last_error_code: Option<String>,
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
            || self.content_revision < 0
            || self.updated_at.timestamp_micros() == 0
            || !valid_optional_timestamp(self.content_synced_at)
            || !valid_optional_timestamp(self.content_last_success_at)
            || !valid_optional_timestamp(self.content_last_error_at)
            || !valid_optional_text(self.content_last_error_code.as_deref(), 64)
            || !valid_optional_text(self.thread_url.as_deref(), 2048)
            || !valid_optional_text(self.title.as_deref(), 512)
            || !valid_optional_multiline_text(self.body.as_deref(), 1_000_000)
        {
            return Err("公开论坛同步投影无效");
        }
        if self.content_last_error_at.is_some() != self.content_last_error_code.is_some() {
            return Err("公开论坛同步错误投影不完整");
        }
        if self.status == "published"
            && (self.content_revision <= 0
                || self.content_synced_at.is_none()
                || [self.thread_url.as_deref(), self.title.as_deref(), self.body.as_deref()]
                    .into_iter()
                    .any(|value| value.is_none_or(str::is_empty)))
        {
            return Err("已发布论坛投影缺少内容");
        }
        Ok(())
    }
}

fn valid_optional_timestamp(value: Option<DateTime<Utc>>) -> bool {
    value.is_none_or(|value| value.timestamp_micros() != 0)
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
