//! 遗留已收款的单 lot 责任认领：只转移既有隔离，不发行旧收入或增加自有额度。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{FundingError, FundingFuture};

/// 唯一卖家义务与原预览绑定的稳定命令；重试不能换 ID 或负载。
#[derive(Debug, Clone, Serialize)]
pub struct LegacyBotSellerCommand {
    pub command_id: Uuid,
    pub actor: String,
    pub obligation_id: Uuid,
    pub expected_uid: String,
    pub expected_state_version: i64,
    pub expected_digest: [u8; 32],
}

impl LegacyBotSellerCommand {
    /// 校验稳定身份与版本形状；实时事实由同一提交事务重新核对。
    pub fn validate(&self) -> Result<(), FundingError> {
        auction_domain::FundingAccountKey::new(&self.expected_uid)
            .map_err(|_| FundingError::Invalid)?;
        if self.command_id.is_nil()
            || self.obligation_id.is_nil()
            || self.actor.is_empty()
            || self.actor.len() > 128
            || self.actor.trim() != self.actor
            || self.actor.chars().any(char::is_control)
            || self.expected_state_version <= 0
            || self.expected_digest == [0; 32]
        {
            return Err(FundingError::Invalid);
        }
        Ok(())
    }

    /// 精确确认仅授权该义务、UID、版本及预览，不代替同会话证据。
    pub fn confirmation(&self) -> String {
        let digest: String =
            self.expected_digest.iter().map(|byte| format!("{byte:02x}")).collect();
        format!(
            "CLAIM LEGACY BOT SELLER {} {} {} {}",
            self.obligation_id, self.expected_uid, self.expected_state_version, digest
        )
    }

    /// 仅稳定授权参与幂等摘要，远端观测时间不改变同一命令。
    pub fn request_digest(&self) -> Result<[u8; 32], FundingError> {
        crate::RequestHash::from_request("legacy_bot_seller_claim_v1", self.obligation_id, self)
            .map(|hash| hash.as_bytes())
            .map_err(|_| FundingError::Invalid)
    }
}

/// 纯数据库投影；精确 Sent 目标来自已成功原投递，CLI 不重建金额或主题。
#[derive(Debug, Clone, Serialize)]
pub struct LegacyBotSellerPreview {
    pub obligation_id: Uuid,
    pub lot_id: Uuid,
    pub payment_obligation_id: Uuid,
    pub payment_delivery_id: Uuid,
    pub payment_external_mail_id: String,
    pub payment_recipient: String,
    pub payment_subject: String,
    pub state_version: i64,
    pub account_version: i64,
    pub account_status: String,
    pub accounting_mode: super::FundingAccountingMode,
    pub policy_version: i64,
    pub buyer_amount: i64,
    pub compensation_amount: i64,
    pub seller_amount: i64,
    pub tax_amount: i64,
    pub quarantine_required: i64,
    pub digest: [u8; 32],
}

impl LegacyBotSellerPreview {
    /// CLI 仅交付原预览与稳定命令；身份/现金/Sent 仍由同一个可信会话取得。
    pub fn evidence_request(
        &self,
        command: &LegacyBotSellerCommand,
        deadline: std::time::Instant,
    ) -> Result<crate::InternalEvidenceRequest, FundingError> {
        command.validate()?;
        if self.obligation_id != command.obligation_id
            || self.state_version != command.expected_state_version
            || self.digest != command.expected_digest
        {
            return Err(FundingError::Conflict);
        }
        Ok(crate::InternalEvidenceRequest {
            operation_id: command.command_id,
            purpose: crate::InternalEvidencePurpose::LegacyBotSeller {
                command_id: command.command_id,
                obligation_id: command.obligation_id,
                expected_state_version: u64::try_from(command.expected_state_version)
                    .map_err(|_| FundingError::Invalid)?,
            },
            world: auction_domain::SettlementWorld::Main,
            equipment: None,
            observe_main_cash: self.accounting_mode == super::FundingAccountingMode::StrictCash,
            deadline,
        })
    }

    /// 旧记录没有可信 sent_at 时保持 None，不用本地 sending_at 冒充远端时间。
    pub fn sent_target(&self) -> crate::InternalSentCollectionTarget {
        crate::InternalSentCollectionTarget {
            external_mail_id: self.payment_external_mail_id.clone(),
            world: auction_domain::SettlementWorld::Main,
            recipient: self.payment_recipient.clone(),
            subject: self.payment_subject.clone(),
            expected_sent_at: None,
        }
    }
}

/// 命令结果仅表达责任留存，不表达新的可消费收入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyBotSellerResult {
    pub command_id: Uuid,
    pub obligation_id: Uuid,
    pub lot_id: Uuid,
    pub uid: String,
    pub retained_credits: i64,
    pub tax_credits: i64,
}

/// 未知事务结果保留 opaque guard；只有以下确定结果可确认结束。
pub enum LegacyBotSellerCommitOutcome {
    Committed(LegacyBotSellerResult),
    /// 已提交账实缺口冻结或保留未知占用；原义务尚未认领，同ID重放返回Unavailable。
    FundingDeferred,
    RolledBack(FundingError),
}

/// 预览/重放只读，认领借用opaque guard快照到事务确定结束，不执行外部调用。
pub trait LegacyBotSellerStore: Send + Sync {
    fn preview_legacy_bot_seller(
        &self,
        obligation_id: Uuid,
        expected_uid: String,
    ) -> FundingFuture<'_, LegacyBotSellerPreview>;
    fn replay_legacy_bot_seller(
        &self,
        command: LegacyBotSellerCommand,
    ) -> FundingFuture<'_, Option<LegacyBotSellerResult>>;
    fn claim_legacy_bot_seller<'a>(
        &'a self,
        command: &'a LegacyBotSellerCommand,
        evidence: &'a crate::SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, LegacyBotSellerCommitOutcome>;
}
