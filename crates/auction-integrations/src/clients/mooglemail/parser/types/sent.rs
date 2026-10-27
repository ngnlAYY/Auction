//! Sent 列表与详情的安全快照模型。

use super::*;

/// 已发送邮件列表中可用于 exact ID 查找的安全摘要。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSentSummary {
    /// 远端 `mid`。
    pub external_mail_id: String,
    pub world: MoogleMailWorld,
    pub recipient: String,
    pub subject: String,
    /// 远端页面按 UTC 展示的时间。
    pub sent_at: DateTime<Utc>,
}

impl fmt::Debug for MoogleMailSentSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSentSummary")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("sent_at", &self.sent_at)
            .finish()
    }
}

impl MoogleMailSentSummary {
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.recipient, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, false)
            && self.sent_at.timestamp_nanos_opt().is_some()
    }
}

/// Sent 详情正文与当前资产的安全 proof；不保存正文，只保存其 canonical digest。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSentContentProof {
    pub external_mail_id: String,
    pub world: MoogleMailWorld,
    pub recipient: String,
    pub subject: String,
    pub body_digest: [u8; 32],
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
}

impl fmt::Debug for MoogleMailSentContentProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSentContentProof")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body_digest", &self.body_digest)
            .field("remaining_credits", &self.remaining_credits)
            .field("cod_pending", &self.cod_pending)
            .field("remaining_equipment_ids", &self.remaining_equipment_ids)
            .finish()
    }
}

impl MoogleMailSentContentProof {
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.recipient, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, false)
            && self.remaining_credits >= 0
            && self.cod_pending >= 0
            && canonical_equipment_ids(&self.remaining_equipment_ids)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSentPage {
    /// 按远端 newest-first 顺序排列的摘要。
    pub summaries: Vec<MoogleMailSentSummary>,
    /// 是否存在严格验证过的下一页链接。
    pub has_next_page: bool,
}

/// 精确 remote ID 的已发送邮件与当前待领取资产快照。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSentSnapshot {
    pub external_mail_id: String,
    pub world: MoogleMailWorld,
    pub recipient: String,
    pub subject: String,
    pub sent_at: DateTime<Utc>,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    /// 当前仍附在邮件中的装备 ID，必须按字典序排列且无重复。
    pub remaining_equipment_ids: Vec<String>,
    /// 页面是否明确证明邮件已领取完毕。
    pub collected: bool,
    pub observed_at: DateTime<Utc>,
}

impl fmt::Debug for MoogleMailSentSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSentSnapshot")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("sent_at", &self.sent_at)
            .field("remaining_credits", &self.remaining_credits)
            .field("cod_pending", &self.cod_pending)
            .field("remaining_equipment_ids", &self.remaining_equipment_ids)
            .field("collected", &self.collected)
            .field("observed_at", &self.observed_at)
            .finish()
    }
}

impl MoogleMailSentSnapshot {
    /// 校验 Sent snapshot 的身份、时间和 remaining/collected 矩阵。
    pub fn validate(&self) -> bool {
        valid_opaque_id(&self.external_mail_id)
            && valid_line(&self.recipient, 256, false)
            && valid_line(&self.subject, MAX_SUMMARY_SUBJECT_BYTES, false)
            && self.sent_at.timestamp_nanos_opt().is_some()
            && self.observed_at.timestamp_nanos_opt().is_some()
            && self.remaining_credits >= 0
            && self.cod_pending >= 0
            && canonical_equipment_ids(&self.remaining_equipment_ids)
            && self.collected
                == (self.remaining_credits == 0
                    && self.cod_pending == 0
                    && self.remaining_equipment_ids.is_empty())
    }
}
