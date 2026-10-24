//! MoogleMail compose 状态和最终发送失败投影。

use std::fmt;

use super::*;

/// 最终 send POST 后可从可见页面文字明确证明的失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoogleMailSendResponseFailure {
    /// 收件人不存在；远端可能同时生成一封需要回收附件的退信。
    RecipientNotFound { returned_mail_id: Option<String> },
    /// 远端明确拒绝向发件账号自身寄信；不表示草稿中的资产已经回收。
    RecipientIsSender,
    /// 远端明确拒绝发送，当前冻结协议只识别邮资不足。
    Rejected,
}

/// 经过结构校验的 compose 页面状态。
/// token 只通过引用提供给下一步 typed form 构造；正文、收件人和附件 key 不进入 Debug 输出。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailComposeState {
    pub(super) token: MoogleMailToken,
    pub(super) recipient: String,
    pub(super) subject: String,
    pub(super) body: String,
    pub(super) equipment_ids: Vec<String>,
    pub(super) credits: i64,
    pub(super) cod: Option<i64>,
    pub(super) attachment_count: usize,
}

impl MoogleMailComposeState {
    pub fn token(&self) -> &MoogleMailToken {
        &self.token
    }

    /// 返回当前页面中的消息字段副本，供下一次 POST 重放。
    pub fn message_fields(&self) -> MoogleMailMessageFields {
        MoogleMailMessageFields {
            recipient: self.recipient.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
        }
    }

    /// 返回已证明的装备 ID，顺序保持页面顺序。
    pub fn equipment_ids(&self) -> &[String] {
        &self.equipment_ids
    }

    pub const fn credits(&self) -> i64 {
        self.credits
    }

    /// 返回 CoD；页面未提供可证明的 CoD 状态时返回 None。
    pub const fn cod(&self) -> Option<i64> {
        self.cod
    }

    pub const fn attachment_count(&self) -> usize {
        self.attachment_count
    }

    /// 判断 compose 是否已回到可安全复用的干净状态。
    pub fn is_clean(&self) -> bool {
        self.recipient.is_empty()
            && self.subject.is_empty()
            && self.body.is_empty()
            && self.equipment_ids.is_empty()
            && self.credits == 0
            && self.attachment_count == 0
            && self.cod == Some(0)
    }
}

impl fmt::Debug for MoogleMailComposeState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailComposeState")
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("equipment_count", &self.equipment_ids.len())
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachment_count)
            .finish()
    }
}

pub(super) const MAX_COMPOSE_ATTACHMENTS: usize = 1000;
pub(super) const MAX_RECIPIENT_BYTES: usize = 256;
pub(super) const MAX_SUBJECT_BYTES: usize = 512;
