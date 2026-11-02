//! 原生 Sent 人工观察的显式责任契约；它不是自动精确附件证明。
//!
//! 两种准入：结果未知（`manual_reconcile`/`write_unknown`）的发送，以及被 Admin 对账为失败、
//! 但原生 Sent 证明实际已寄出的发送。后者只追加更正事实，原失败命令与审计保持不变。

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApplicationError, ApplicationFuture, DeliverySendExpectation, RequestHash};

/// 操作者保存的实际观察；已收取附件必须保持为空，不能填入期待 EID。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualSentEvidence {
    pub schema_version: u16,
    pub world_id: String,
    pub external_mail_id: String,
    pub sender_uid: String,
    pub sender_name: String,
    pub recipient: String,
    pub subject: String,
    pub sent_minute: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub original_body_digest: [u8; 32],
    pub observed_body_digest: [u8; 32],
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub equipment_ids: Vec<String>,
    pub collected: bool,
    pub receipt: String,
}

impl ManualSentEvidence {
    /// 仅验证规范形状；冻结身份、资金 UID 和分钟交集由最终事务核验。
    pub fn validate(&self) -> Result<(), ApplicationError> {
        if self.schema_version != 1
            || self.world_id != "main"
            || !auction_domain::valid_positive_decimal(&self.external_mail_id, 64)
            || !auction_domain::valid_positive_decimal(&self.sender_uid, 64)
            || !crate::valid_mail_player_name(&self.sender_name)
            || !crate::valid_mail_player_name(&self.recipient)
            || !auction_domain::valid_text(&self.subject, 512)
            || self.sent_minute.second() != 0
            || self.sent_minute.nanosecond() != 0
            || self.sent_minute.timestamp_nanos_opt().is_none()
            || self.observed_at.timestamp_nanos_opt().is_none()
            || self.observed_at < self.sent_minute
            || self.original_body_digest == [0; 32]
            || self.observed_body_digest == [0; 32]
            || !(0..=crate::MAX_DELIVERY_COD).contains(&self.remaining_credits)
            || !(0..=crate::MAX_DELIVERY_COD).contains(&self.cod_pending)
            || self.equipment_ids.len() > 1
            || self.equipment_ids.iter().any(|id| !auction_domain::valid_positive_decimal(id, 64))
            || self.receipt.len() > 2048
        {
            return Err(ApplicationError::FulfillmentDependencyMismatch);
        }
        Ok(())
    }

    /// 现金附件未领取时须原额仍在；已领取时只接受与冻结金额一致的原生回执，并由命令显式
    /// 接受领取责任。装备收取回执同样需命令显式接受缺失 EID 的责任。
    pub fn matches_frozen(&self, expected: &DeliverySendExpectation, item_name: &str) -> bool {
        if !expected.validate()
            || expected.world_id != self.world_id
            || expected.recipient != self.recipient
            || expected.subject != self.subject
            || expected.body_digest != self.original_body_digest
        {
            return false;
        }
        if expected.credits > 0 {
            if expected.cod != 0
                || !expected.equipment_ids.is_empty()
                || self.cod_pending != 0
                || !self.equipment_ids.is_empty()
            {
                return false;
            }
            if !self.collected {
                return self.receipt.is_empty()
                    && self.observed_body_digest == self.original_body_digest
                    && self.remaining_credits == expected.credits;
            }
            // 附件现金在发送时已离开账户，领取只改变收件人侧；回执金额必须与冻结金额逐字一致。
            return self.remaining_credits == 0
                && self.observed_body_digest != self.original_body_digest
                && self.receipt
                    == format!(
                        "Removed attachment: {} Credits",
                        grouped_credits(expected.credits)
                    );
        }
        if expected.credits != 0
            || expected.equipment_ids.len() != 1
            || !auction_domain::valid_text(item_name, 512)
        {
            return false;
        }
        if !self.collected {
            return self.receipt.is_empty()
                && self.observed_body_digest == self.original_body_digest
                && self.remaining_credits == expected.credits
                && self.cod_pending == expected.cod
                && self.equipment_ids == expected.equipment_ids;
        }
        let receipt = if expected.cod > 0 {
            format!(
                "CoD Paid: {} Credits\n\nRemoved attachment: {item_name}",
                grouped_credits(expected.cod)
            )
        } else {
            format!("Removed attachment: {item_name}")
        };
        self.remaining_credits == 0
            && self.cod_pending == 0
            && self.equipment_ids.is_empty()
            && self.observed_body_digest != self.original_body_digest
            && self.receipt == receipt
    }
}

fn grouped_credits(amount: i64) -> String {
    let digits = amount.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(char::from(digit));
    }
    grouped
}

/// 稳定命令绑定原观察、当前 fence 和两项独立人工责任。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentConfirmationCommand {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub expected_fencing_version: i64,
    pub actor: String,
    pub reason: String,
    pub evidence: ManualSentEvidence,
    pub accept_collected_assets: bool,
    pub accept_attempt_history: bool,
}

impl SentConfirmationCommand {
    pub fn validate(&self) -> Result<(), ApplicationError> {
        self.evidence.validate()?;
        if self.command_id.is_nil()
            || self.delivery_id.is_nil()
            || self.expected_fencing_version <= 0
            || !auction_domain::valid_text(&self.actor, 128)
            || !auction_domain::valid_text(&self.reason, 2048)
        {
            return Err(ApplicationError::FulfillmentDependencyMismatch);
        }
        Ok(())
    }

    pub fn request_digest(&self) -> Result<[u8; 32], ApplicationError> {
        RequestHash::from_request("delivery_sent_confirmation_v1", self.command_id, self)
            .map(RequestHash::as_bytes)
            .map_err(ApplicationError::from)
    }
}

/// 无副作用准入快照；必须由最终事务重新读取，不能把预览当作成功证明。
#[derive(Debug, Clone, Serialize)]
pub struct SentConfirmationPreview {
    pub eligible: bool,
    pub blockers: Vec<String>,
    pub delivery_id: Uuid,
    pub expected_fencing_version: i64,
    pub external_reference: String,
    pub requires_collected_asset_acceptance: bool,
    pub requires_attempt_history_acceptance: bool,
    /// 目标是被 Admin 对账为失败的投递；确认即以 Sent 证据更正该人工失败结论。
    pub corrects_admin_failure: bool,
}

/// 原命令结果永久保存，同 ID 同参重放不重复追加事实。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentConfirmationResult {
    pub command_id: Uuid,
    pub delivery_id: Uuid,
    pub external_reference: String,
    pub replayed: bool,
}

/// 只向正式运维 CLI 开放；Admin 原始手填成功禁止保持不变。
pub trait DeliverySentConfirmationPort: Send + Sync {
    fn preview_sent_confirmation(
        &self,
        delivery_id: Uuid,
        evidence: ManualSentEvidence,
    ) -> ApplicationFuture<'_, SentConfirmationPreview>;
    fn confirm_sent(
        &self,
        command: SentConfirmationCommand,
    ) -> ApplicationFuture<'_, SentConfirmationResult>;
}
