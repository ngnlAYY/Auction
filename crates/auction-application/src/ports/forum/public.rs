//! Public 论坛镜像读取端口。
//! 该端口只读取本地已经持久化的论坛同步投影，不直接调用论坛网关，也不把远程发布成功
//! 推断为本地状态。未接入的后端必须显式报告能力缺失。

use chrono::{DateTime, Utc};
use url::Url;
use uuid::Uuid;

use crate::ports::auction::ApplicationFuture;

/// Public 论坛同步状态的闭集。
pub const PUBLIC_FORUM_STATUSES: [&str; 4] = ["pending", "publishing", "published", "failed"];

/// 通过已发布内容证明与地址校验的窄投影；私有字段阻止裸 URL 绕过校验进入公开 DTO。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicForumLink {
    url: String,
}

impl PublicForumLink {
    /// 只接受正内容版本、有效同步时间及有界安全 HTTPS 地址。
    pub fn new(
        url: String,
        content_revision: i64,
        content_synced_at: Option<DateTime<Utc>>,
    ) -> Result<Self, &'static str> {
        validate_forum_content_proof(content_revision, content_synced_at)?;
        if !valid_forum_url(&url) {
            return Err("公开论坛主题地址无效");
        }
        Ok(Self { url })
    }

    /// 将已经校验的地址移交 HTTP DTO，不重新分配或规范化原始地址。
    pub fn into_url(self) -> String {
        self.url
    }
}

/// Public 论坛镜像安全投影。
/// 已发布正文仍可能包含完整装备 URL，不能因进入只读镜像而恢复明文调试输出。
#[derive(Clone, PartialEq, Eq)]
pub struct PublicForumThread {
    pub auction_id: Uuid,
    pub status: String,
    pub source_revision: i64,
    pub thread_url: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
    /// 全部目录页及系统日志的本地 CAS 版本；不暴露 Worker claim 或下次调度时间。
    pub content_revision: i64,
    /// 最近一次由完整主题读取证明已处理到的真实论坛楼层。
    /// 这是当前观察投影，删帖后可下降；不可用作稳定命令身份或事实 sequence。
    pub processed_through_post_number: i32,
    pub content_synced_at: Option<DateTime<Utc>>,
    pub content_last_success_at: Option<DateTime<Utc>>,
    /// 当前未恢复扫描错误的发生时间。
    pub content_last_error_at: Option<DateTime<Utc>>,
    /// 当前未恢复扫描错误的有界稳定码，不包含远端正文。
    pub content_last_error_code: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for PublicForumThread {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicForumThread")
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

impl PublicForumThread {
    /// 校验同步状态及已发布内容的完整性。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.auction_id.is_nil()
            || !PUBLIC_FORUM_STATUSES.contains(&self.status.as_str())
            || self.status != self.status.trim()
            || self.status.chars().any(char::is_control)
            || self.source_revision < 0
            || self.content_revision < 0
            || self.processed_through_post_number < 2
            || self.updated_at.timestamp_micros() == 0
            || !valid_optional_timestamp(self.content_synced_at)
            || !valid_optional_timestamp(self.content_last_success_at)
            || !valid_optional_timestamp(self.content_last_error_at)
            || !auction_domain::valid_optional_text(
                self.content_last_error_code.as_deref(),
                64,
                false,
            )
            || !auction_domain::valid_optional_text(self.title.as_deref(), 512, false)
            || !valid_optional_multiline_text(self.body.as_deref(), 1_000_000)
        {
            return Err("公开论坛同步投影无效");
        }
        if self.content_last_error_at.is_some() != self.content_last_error_code.is_some() {
            return Err("公开论坛同步错误投影不完整");
        }
        if self.status == "published" {
            let url = self.thread_url.as_deref().ok_or("已发布论坛投影缺少内容")?;
            if !valid_forum_url(url) {
                return Err("公开论坛主题地址无效");
            }
            validate_forum_content_proof(self.content_revision, self.content_synced_at)?;
            if [self.title.as_deref(), self.body.as_deref()]
                .into_iter()
                .any(|value| value.is_none_or(str::is_empty))
            {
                return Err("已发布论坛投影缺少内容");
            }
        } else if !self.thread_url.as_deref().is_none_or(valid_forum_url) {
            return Err("公开论坛同步投影无效");
        }
        Ok(())
    }
}

/// 历史 published 行可能尚无固定楼层内容证明；不能仅凭状态公开主题地址。
fn validate_forum_content_proof(
    content_revision: i64,
    content_synced_at: Option<DateTime<Utc>>,
) -> Result<(), &'static str> {
    if content_revision <= 0 || content_synced_at.is_none_or(|value| value.timestamp_micros() == 0)
    {
        return Err("公开论坛内容证明无效");
    }
    Ok(())
}

fn valid_forum_url(value: &str) -> bool {
    auction_domain::valid_optional_text(Some(value), 2048, false)
        && Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

fn valid_optional_timestamp(value: Option<DateTime<Utc>>) -> bool {
    value.is_none_or(|value| value.timestamp_micros() != 0)
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

pub trait PublicForumReader: Send + Sync {
    /// 读取仍在运行的 Worker 公布的启用状态；没有可靠观测时返回 None，不能猜测为关闭。
    fn public_forum_sync_enabled(&self) -> ApplicationFuture<'_, Option<bool>>;

    fn public_forum_thread(
        &self,
        auction_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicForumThread>>;
}
