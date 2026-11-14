//! 收件箱摘要、详情 envelope 和动作前后状态。

use std::collections::HashSet;
use std::fmt;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use super::MAX_SUMMARY_SUBJECT_BYTES;
use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::parser::{valid_message_body, write_digest_string};
use crate::clients::mooglemail::protocol::{
    MoogleMailEquipmentKey, MoogleMailToken, valid_decimal_id, valid_hex_key, valid_line,
    valid_opaque_id,
};

/// 收件箱列表中不含正文和附件的安全摘要。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSummary {
    /// 远端 `mid`。
    pub external_mail_id: String,
    pub world: MoogleMailWorld,
    pub sender: String,
    pub subject: String,
    /// 远端页面按 UTC 展示的时间。
    pub received_at: DateTime<Utc>,
}

impl fmt::Debug for MoogleMailSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSummary")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("received_at", &self.received_at)
            .finish()
    }
}

impl MoogleMailSummary {
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.sender, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, true)
            && self.received_at.timestamp_nanos_opt().is_some()
    }
}

/// 一件 MoogleMail 装备附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailAttachment {
    pub equipment_id: String,
    /// 读取装备页所需的短生命周期 key；默认脱敏。
    pub key: MoogleMailEquipmentKey,
}

/// 通过摘要与详情双重校验后的 typed 邮件 envelope。
/// 该类型不保存 HTML、mmtoken 或 Cookie；邮件正文可能包含敏感内容，装备 key 属于公开游戏链接。
/// 两者仍按现有短生命周期处理，在应用处理完成后尽快丢弃或调用 `clear_secrets`。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailMessage {
    pub external_mail_id: String,
    pub world: MoogleMailWorld,
    pub received_at: DateTime<Utc>,
    pub sender: String,
    pub subject: String,
    pub body: String,
    pub credits: i64,
    pub cod: i64,
    pub attachments: Vec<MoogleMailAttachment>,
    pub returnable: bool,
    pub claimable: bool,
}

/// 详情页动作编排所需的短生命周期状态。
/// 该状态为动作前后身份与资产证明保留 token、邮件身份、正文和附件清单，
/// 但不保存装备 key 或原始 HTML。`Debug` 对身份与正文字段脱敏。
pub struct MoogleMailActionState {
    pub(crate) token: MoogleMailToken,
    pub(crate) recipient: String,
    pub(crate) sender: String,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) equipment_ids: Vec<String>,
    pub(crate) credits: i64,
    pub(crate) cod: i64,
    pub(crate) attachment_count: usize,
    pub returnable: bool,
    pub claimable: bool,
}

impl fmt::Debug for MoogleMailActionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailActionState")
            .field("token", &"[REDACTED]")
            .field("recipient", &"[REDACTED]")
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("equipment_count", &self.equipment_ids.len())
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachment_count)
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .finish()
    }
}

impl MoogleMailActionState {
    pub(crate) fn token(&self) -> &MoogleMailToken {
        &self.token
    }

    pub(crate) fn sender(&self) -> &str {
        &self.sender
    }

    pub(crate) fn equipment_ids(&self) -> &[String] {
        &self.equipment_ids
    }

    pub(crate) fn credits(&self) -> i64 {
        self.credits
    }

    pub(crate) fn cod(&self) -> i64 {
        self.cod
    }

    pub(crate) fn attachment_count(&self) -> usize {
        self.attachment_count
    }
}

impl fmt::Debug for MoogleMailMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailMessage")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("received_at", &self.received_at)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("attachment_count", &self.attachments.len())
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .finish()
    }
}

impl MoogleMailMessage {
    /// 校验 envelope 的稳定字段、不变量和附件唯一性。
    pub fn validate(&self) -> bool {
        if !valid_opaque_id(&self.external_mail_id)
            || self.received_at.timestamp_nanos_opt().is_none()
            || !valid_line(&self.sender, 256, false)
            || !valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, true)
            || !valid_message_body(&self.body)
            || self.credits < 0
            || self.cod < 0
            || (self.cod > 0 && self.credits == 0 && self.attachments.is_empty())
        {
            return false;
        }
        let mut seen = HashSet::with_capacity(self.attachments.len());
        self.attachments.iter().all(|attachment| {
            valid_decimal_id(&attachment.equipment_id)
                && valid_hex_key(attachment.key.expose())
                && seen.insert(attachment.equipment_id.as_str())
        })
    }

    /// 返回用于入站幂等和审计关联的稳定语义摘要。
    pub fn content_digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        write_digest_string(&mut digest, "mooglemail-envelope-v1");
        let received_at = self.received_at.to_rfc3339();
        let sender = self.sender.to_ascii_lowercase();
        for value in [
            self.external_mail_id.as_str(),
            self.world.as_str(),
            received_at.as_str(),
            sender.as_str(),
            self.subject.as_str(),
            self.body.as_str(),
        ] {
            write_digest_string(&mut digest, value);
        }
        digest.update(self.credits.to_be_bytes());
        digest.update(self.cod.to_be_bytes());
        digest.update([u8::from(self.returnable), u8::from(self.claimable)]);
        // 附件身份已由 envelope 校验保证唯一；直接排序借用，避免每个 ID 再扫描整组附件。
        let mut attachments = self.attachments.iter().collect::<Vec<_>>();
        attachments.sort_unstable_by(|left, right| left.equipment_id.cmp(&right.equipment_id));
        for attachment in attachments {
            write_digest_string(&mut digest, &attachment.equipment_id);
            digest.update(attachment.key.expose().as_bytes());
        }
        let bytes = digest.finalize();
        let mut result = [0_u8; 32];
        result.copy_from_slice(&bytes);
        result
    }

    /// 尽力清除当前 envelope 持有的装备 key；正文由所有权 drop 回收。
    pub fn clear_secrets(&mut self) {
        for attachment in &mut self.attachments {
            attachment.key.clear();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailInboxPage {
    /// 按远端 newest-first 顺序排列的摘要。
    pub summaries: Vec<MoogleMailSummary>,
    /// 是否存在严格验证过的下一页链接。
    pub has_next_page: bool,
}
