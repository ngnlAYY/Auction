//! 同认证会话的精确 Sent 领取观察；原 COD 金额只能由持久成功 payload/义务证明。

use auction_domain::SettlementWorld;
use chrono::{DateTime, Utc};

use super::model::facts_digest;
use crate::ports::core::lower_hex;
use crate::ports::settlement::MailAccountEvidenceBinding;

/// 来自已证明成功 delivery 的精确目标，不接受以本次读值反填“预期”。
/// 旧成功记录未保存远端 sent_at 时传 None，禁止用 sending_at/当前时钟冒充。
#[derive(Debug)]
pub struct InternalSentCollectionTarget {
    pub external_mail_id: String,
    pub world: SettlementWorld,
    pub recipient: String,
    pub subject: String,
    pub expected_sent_at: Option<DateTime<Utc>>,
}

impl InternalSentCollectionTarget {
    /// 这里只校验读取参数，关联原 delivery/义务和冻结 COD 由事务 owner 核验。
    pub fn validate(&self) -> bool {
        valid_line(&self.external_mail_id, 256)
            && crate::ports::admin::valid_mail_player_name(&self.recipient)
            && valid_line(&self.subject, 512)
    }
}

/// 当前 UID 的同一 session 中，列表/详情自洽且精确目标已领取的安全观察。
/// 不包含原金额；零 remaining/COD 是当前状态，不能由它猜原实收金额或来源。
#[derive(Debug)]
pub struct VerifiedMailSentCollectionObservation {
    pub binding: MailAccountEvidenceBinding,
    pub external_mail_id: String,
    pub recipient: String,
    pub subject: String,
    pub sent_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
    pub collected: bool,
    pub digest: [u8; 32],
}

impl VerifiedMailSentCollectionObservation {
    /// 固定协议的领取状态与同账号绑定摘要；无邮件正文、Cookie 或臆测的原金额。
    pub fn compute_digest(&self) -> [u8; 32] {
        // 直接串联借用字段；有效证明只允许空列表，摘要仍覆盖无效输入的完整列表。
        facts_digest(
            [
                "same_session_exact_sent_collected_v1",
                &lower_hex(&self.binding.digest),
                &self.external_mail_id,
                &self.recipient,
                &self.subject,
                &self.sent_at.to_rfc3339(),
                &self.observed_at.to_rfc3339(),
                &self.remaining_credits.to_string(),
                &self.cod_pending.to_string(),
                &self.collected.to_string(),
                &self.remaining_equipment_ids.len().to_string(),
            ]
            .into_iter()
            .chain(self.remaining_equipment_ids.iter().map(String::as_str)),
        )
    }

    /// 只接纳明确全部领取；原资金化授权还须锁验成功 payload 和唯一义务来源。
    pub fn validate(&self) -> bool {
        self.binding.validate()
            && valid_line(&self.external_mail_id, 256)
            && crate::ports::admin::valid_mail_player_name(&self.recipient)
            && valid_line(&self.subject, 512)
            && self.binding.observed_at <= self.observed_at
            && self.sent_at <= self.observed_at
            && self.collected
            && self.remaining_credits == 0
            && self.cod_pending == 0
            && self.remaining_equipment_ids.is_empty()
            && self.digest == self.compute_digest()
    }
}

fn valid_line(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
