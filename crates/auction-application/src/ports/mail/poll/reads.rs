//! 详情 GET 可能改变远端已读状态；先持久登记 MID 与列表身份，再读取详情。

use std::fmt;

use auction_domain::valid_text;
use auction_time::Timestamp;

use crate::ports::core::ApplicationFuture;
use crate::ports::mail::MailPollClaim;

/// 恢复详情读取所需的定位与列表快照；不含正文、HTML、Cookie、token 或装备 key。
#[derive(Clone, PartialEq, Eq)]
pub struct MailPollReadSummary {
    pub external_mail_id: String,
    pub world_id: String,
    /// 首次列表显示名可能被截断，仅保留恢复快照兼容；详情身份与白名单不得使用该值。
    pub sender: String,
    pub subject: String,
    pub received_at: Timestamp,
    /// 列表页声明的世界标记；旧版登记的行为空，恢复时须按门控决定是否允许继续。
    pub list_world_marker: Option<String>,
    /// 列表页原始分钟文本；旧版登记的行为空。
    pub received_minute_raw: Option<String>,
}

impl MailPollReadSummary {
    /// 无效身份不得进入持久队列或直接 MID 读取路径。
    pub fn validate(&self) -> bool {
        valid_text(&self.external_mail_id, 256)
            && matches!(self.world_id.as_str(), "main" | "isekai")
            && valid_text(&self.sender, 256)
            && self.subject.len() <= 512
            && self.subject == self.subject.trim()
            && !self.subject.chars().any(char::is_control)
            && self.received_at.timestamp_nanos_opt().is_some()
            && self
                .list_world_marker
                .as_deref()
                .is_none_or(|marker| matches!(marker, "main" | "isekai"))
            && self.received_minute_raw.as_deref().is_none_or(|minute| valid_text(minute, 64))
            && (self.received_minute_raw.is_some() || self.list_world_marker.is_none())
    }
}

impl fmt::Debug for MailPollReadSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailPollReadSummary")
            .field("external_mail_id", &self.external_mail_id)
            .field("world_id", &self.world_id)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("received_at", &self.received_at)
            .field("list_world_marker", &self.list_world_marker)
            .field("has_received_minute_raw", &self.received_minute_raw.is_some())
            .finish()
    }
}

/// 详情读取的持久恢复队列；全部操作校验当前 claim 的邮箱、token、owner、版本及实时租约。
/// `stage` 按处理顺序原子登记整窗，提交完成前缀时由 MailPollStore 在同一事务删除对应前缀。
pub trait MailPollReadJournal: Send + Sync {
    /// 优先返回同一 mailbox/world 尚未提交的最早前缀；已有其他 world 的队列须失败关闭。
    fn pending_reads(
        &self,
        claim: MailPollClaim,
        world_id: String,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<MailPollReadSummary>>;

    /// 详情 GET 前持久保存完整窗口；重复 MID 必须验证原身份一致，不能覆盖旧摘要。
    fn stage_reads(
        &self,
        claim: MailPollClaim,
        world_id: String,
        summaries: Vec<MailPollReadSummary>,
    ) -> ApplicationFuture<'_, ()>;
}
