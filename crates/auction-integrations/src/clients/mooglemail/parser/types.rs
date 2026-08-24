use super::*;

pub(crate) const MAX_INBOX_ROWS_PER_PAGE: usize = 1000;
pub(crate) const MAX_SENT_ROWS_PER_PAGE: usize = MAX_INBOX_ROWS_PER_PAGE;
pub(crate) const MAX_ATTACHMENT_ENTRIES: usize = 1000;
pub(crate) const MAX_SUMMARY_SUBJECT_BYTES: usize = 512;
pub(crate) const MAX_MESSAGE_TEXT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const INBOX_TIME_FORMAT: &str = "%Y-%m-%d %H:%M";

/// 收件箱列表中不含正文和附件的安全摘要。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSummary {
    /// 远端 `mid`。
    pub external_mail_id: String,
    /// 邮件所在世界。
    pub world: MoogleMailWorld,
    /// 发件人显示名。
    pub sender: String,
    /// 收件箱标题。
    pub subject: String,
    /// 远端 HVT-8 时间转换后的 UTC 时间。
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
    /// 校验摘要可以进入详情读取队列。
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
    /// 远端装备 ID。
    pub equipment_id: String,
    /// 读取装备页所需的短生命周期 key；默认脱敏。
    pub key: MoogleMailEquipmentKey,
}

/// 通过摘要与详情双重校验后的 typed 邮件 envelope。
///
/// 该类型不保存 HTML、mmtoken 或 Cookie，但正文和装备 key 仍属于敏感的短生命周期数据，
/// 只能在应用处理完成后尽快丢弃或调用 `clear_secrets`。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailMessage {
    /// 远端邮件 ID。
    pub external_mail_id: String,
    /// 邮件世界。
    pub world: MoogleMailWorld,
    /// 收件时间。
    pub received_at: DateTime<Utc>,
    /// 认证详情页中的发件人。
    pub sender: String,
    /// 认证详情页中的标题。
    pub subject: String,
    /// 认证详情页 textarea 正文。
    pub body: String,
    /// 附带的 Credits 数量。
    pub credits: i64,
    /// 附带的 COD 数量。
    pub cod: i64,
    /// 附带装备。
    pub attachments: Vec<MoogleMailAttachment>,
    /// 详情页是否提供整封退回动作证明。
    pub returnable: bool,
    /// 详情页是否提供领取附件动作证明。
    pub claimable: bool,
}

/// 详情页动作编排所需的短生命周期状态。
///
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
    /// 详情页是否声明可以退回整封邮件。
    pub returnable: bool,
    /// 详情页是否声明可以领取全部附件。
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
        let mut equipment_ids = self
            .attachments
            .iter()
            .map(|attachment| attachment.equipment_id.as_str())
            .collect::<Vec<_>>();
        equipment_ids.sort_unstable();
        for equipment_id in equipment_ids {
            write_digest_string(&mut digest, equipment_id);
            if let Some(attachment) =
                self.attachments.iter().find(|item| item.equipment_id == equipment_id)
            {
                digest.update(attachment.key.expose().as_bytes());
            }
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

/// 单页收件箱解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailInboxPage {
    /// 按远端 newest-first 顺序排列的摘要。
    pub summaries: Vec<MoogleMailSummary>,
    /// 是否存在严格验证过的下一页链接。
    pub has_next_page: bool,
}

/// 已发送邮件列表中可用于 exact ID 查找的安全摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentSummary {
    /// 远端 `mid`。
    pub external_mail_id: String,
    /// 邮件所在世界。
    pub world: MoogleMailWorld,
    /// 收件人显示名。
    pub recipient: String,
    /// 已发送邮件标题。
    pub subject: String,
    /// 远端 HVT-8 时间转换后的 UTC 时间。
    pub sent_at: DateTime<Utc>,
}

impl MoogleMailSentSummary {
    /// 校验摘要可以进入 Sent 详情读取队列。
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.recipient, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, false)
            && self.sent_at.timestamp_nanos_opt().is_some()
    }
}

/// 单页已发送邮件解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentPage {
    /// 按远端 newest-first 顺序排列的摘要。
    pub summaries: Vec<MoogleMailSentSummary>,
    /// 是否存在严格验证过的下一页链接。
    pub has_next_page: bool,
}

/// 精确 remote ID 的已发送邮件与当前待领取资产快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentSnapshot {
    /// 远端邮件 ID。
    pub external_mail_id: String,
    /// 邮件所在世界。
    pub world: MoogleMailWorld,
    /// 收件人。
    pub recipient: String,
    /// 标题。
    pub subject: String,
    /// 发送时间。
    pub sent_at: DateTime<Utc>,
    /// 当前仍附在邮件中的 Credits。
    pub remaining_credits: i64,
    /// 当前仍待支付的 COD。
    pub cod_pending: i64,
    /// 当前仍附在邮件中的装备 ID，必须按字典序排列且无重复。
    pub remaining_equipment_ids: Vec<String>,
    /// 页面是否明确证明邮件已领取完毕。
    pub collected: bool,
    /// 本次读取的观察时间。
    pub observed_at: DateTime<Utc>,
}

impl MoogleMailSentSnapshot {
    /// 校验 Sent snapshot 的身份、时间和 remaining/collected 矩阵。
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.recipient, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, false)
            && self.sent_at.timestamp_nanos_opt().is_some()
            && self.observed_at.timestamp_nanos_opt().is_some()
            && !self.observed_at.lt(&self.sent_at)
            && self.remaining_credits >= 0
            && self.cod_pending >= 0
            && canonical_equipment_ids(&self.remaining_equipment_ids)
            && self.collected
                == (self.remaining_credits == 0
                    && self.cod_pending == 0
                    && self.remaining_equipment_ids.is_empty())
    }
}

/// 一次同步读取的 world wallet Credits 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailBalanceSnapshot {
    /// 余额所属世界。
    pub world: MoogleMailWorld,
    /// 页面与脚本均证明的 Credits 余额。
    pub credits: i64,
    /// 本次读取的观察时间。
    pub observed_at: DateTime<Utc>,
}

impl MoogleMailBalanceSnapshot {
    /// 校验余额 world、数值和观察时间。
    pub fn validate(&self) -> bool {
        self.credits >= 0 && self.observed_at.timestamp_nanos_opt().is_some()
    }
}
