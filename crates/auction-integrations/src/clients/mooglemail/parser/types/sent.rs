//! Sent 列表与详情的安全快照模型。

use std::fmt;

use auction_domain::consignment::RemovedMarker;
use auction_time::Timestamp;

use super::MAX_SUMMARY_SUBJECT_BYTES;
use crate::clients::mooglemail::MoogleMailWorld;
use crate::clients::mooglemail::parser::canonical_equipment_ids;
use crate::clients::mooglemail::protocol::{valid_line, valid_opaque_id};

/// 已发送邮件列表中可用于 exact ID 查找的安全摘要。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSentSummary {
    /// 远端 `mid`。
    pub external_mail_id: String,
    pub world: MoogleMailWorld,
    pub recipient: String,
    pub subject: String,
    /// 远端页面按 UTC 展示的时间。
    pub sent_at: Timestamp,
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

/// Sent 详情正文末尾的服务端回执块（领取后由 HV 追加）；只描述形状，不判定业务含义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoogleMailSentReceipt {
    /// 正文末尾没有回执块。
    None,
    /// 回执块格式清楚：回执前的原正文摘要、每个附件的回执行（Credits 附件写作 `N Credits`）与 `CoD Paid` 金额。
    Parsed { original_body_digest: [u8; 32], removed: Vec<RemovedMarker>, cod_paid: Option<i64> },
    /// 有回执块但格式不符（marker 重复、名称为空、CoD Paid 多于一行或金额格式不符）。
    Malformed,
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
    /// 正文末尾的服务端回执块；`body_digest` 仍是整页正文摘要，不受回执块影响。
    pub receipt: MoogleMailSentReceipt,
    /// 正文是否含系统退回附言（`This message was returned from`）。
    pub returned_notice: bool,
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
            .field("returned_notice", &self.returned_notice)
            .finish_non_exhaustive()
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
    pub sent_at: Timestamp,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    /// 当前仍附在邮件中的装备 ID，必须按字典序排列且无重复。
    pub remaining_equipment_ids: Vec<String>,
    /// 页面是否明确证明邮件已领取完毕。
    pub collected: bool,
    pub observed_at: Timestamp,
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
