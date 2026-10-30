//! 资金用途授权、只读责任预览及同事务资金动作的安全契约。

use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};
use uuid::Uuid;

#[path = "funding/policy.rs"]
mod policy;
#[path = "funding/record_only.rs"]
mod record_only;
pub use policy::{FundingAccountingMode, FundingPolicy, FundingPolicyReader};
pub use record_only::{FundingRecordOnlyCommand, FundingRecordOnlyStore};

/// 资金错误保持低基数，不回显数据库或远端正文。
#[derive(Debug, thiserror::Error)]
pub enum FundingError {
    #[error("资金输入或冻结事实无效")]
    Invalid,
    #[error("资金版本、来源或命令冲突")]
    Conflict,
    #[error("账户未对账、已冻结或资金不足")]
    Unavailable,
    #[error("责任或外部结果尚未证明")]
    Unproven,
    #[error("完整现金发送门尚未接线")]
    Unsupported,
    #[error("资金事务序列化冲突")]
    SerializationConflict,
    #[error("资金存储失败；可能提交的命令必须按原ID核对")]
    Storage,
}

/// 新窄端口使用独立 future，不扩张旧 delivery trait。
pub type FundingFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, FundingError>> + Send + 'a>>;

/// 当前构建完整发送门能力；不代替停止旧Worker/同版升级的运维条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingCashGate {
    V1,
}

/// 只有真实已接线的 delivery owner 才提供 V1；兼容实现默认不支持。
pub trait FundingCashGateProvider: Send + Sync {
    fn funding_cash_gate(&self) -> Option<FundingCashGate> {
        None
    }
}

/// 稳定业务身份，不将可变远端观测时间放入重放摘要。
#[derive(Debug, Clone)]
pub struct FundingMutation {
    pub command_id: Uuid,
    pub request_digest: [u8; 32],
}

/// 外发未知保留全部占用，不以失败或超时推导返还。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingDeliveryOutcome {
    Sent,
    Unknown,
}

/// 同一排他session精确Sent-collected的安全实收绑定；不接受Balance替代。
#[derive(Debug)]
pub struct FundingCollectedReceipt<'a> {
    pub payment_obligation_id: Uuid,
    pub delivery_id: Uuid,
    pub buyer_credits: i64,
    /// 借用A同一次prepared/commit guard快照；record_only保留唯一Sent观察并省略Main余额。
    pub evidence: &'a crate::SettlementInternalEvidenceSnapshot,
}

/// 只读预览含全部责任投影，不含秘密、邮件正文或管理员提供的余额。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingPreview {
    pub expected_uid: String,
    pub account_version: i64,
    pub account_status: String,
    pub allocated_local: i64,
    pub isolated_liabilities: i64,
    pub unfunded_compensation: i64,
    pub blockers: Vec<String>,
    pub facts: serde_json::Value,
    pub digest: [u8; 32],
}

/// 管理员只授权用途和目标断言；真实UID/现金由证据端口产生。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingReconciliationCommand {
    pub command_id: Uuid,
    pub actor: String,
    pub expected_uid: String,
    pub expected_version: i64,
    pub preview_digest: [u8; 32],
    pub proprietary_credits: i64,
    pub compensation_credits: i64,
    pub confirm: String,
    pub dedicated_account_no_external_liabilities: bool,
}

impl FundingReconciliationCommand {
    /// 精确确认绑定UID、版本、两类额度和预览；声明不替代远端事实。
    pub fn confirmation(&self) -> String {
        let digest = crate::ports::lower_hex(&self.preview_digest);
        format!(
            "RECONCILE BOT FUNDS {} {} {} {} {}",
            self.expected_uid,
            self.expected_version,
            self.proprietary_credits,
            self.compensation_credits,
            digest
        )
    }

    pub fn validate(&self) -> Result<(), FundingError> {
        auction_domain::FundingAccountKey::new(&self.expected_uid)
            .map_err(|_| FundingError::Invalid)?;
        if self.command_id.is_nil()
            || self.actor.is_empty()
            || self.actor.len() > 128
            || self.actor.trim() != self.actor
            || self.actor.chars().any(char::is_control)
            || self.expected_version < 0
            || self.proprietary_credits < 0
            || self.compensation_credits < 0
            || !self.dedicated_account_no_external_liabilities
            || self.confirm != self.confirmation()
            || self.preview_digest == [0; 32]
        {
            return Err(FundingError::Invalid);
        }
        Ok(())
    }
}

/// 保存原结果，重放不再创建source或递增账户。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingReconciliationResult {
    pub command_id: Uuid,
    pub uid: String,
    pub account_version: i64,
    pub source_id: Option<Uuid>,
    pub account_status: String,
    pub allocated_credits: i64,
}

/// 数据库已确定提交或回滚；Err只用于提交/回滚结果未知。
#[derive(Debug)]
pub enum FundingCommitOutcome {
    Committed(FundingReconciliationResult),
    RolledBack(FundingError),
}

/// 安全快照借用到事务结束，不能detach；实现负责最多三次Serializable重试。
pub trait FundingReconciliationWriter: Send + Sync {
    fn reconcile_funding<'a>(
        &'a self,
        command: &'a FundingReconciliationCommand,
        evidence: &'a crate::SettlementInternalEvidenceSnapshot,
        gate: FundingCashGate,
    ) -> FundingFuture<'a, FundingCommitOutcome>;
}

/// 预览和原命令查询严格只读；提交接口待真实证据通过后调用。
pub trait FundingReconciliationReader: Send + Sync {
    fn preview_funding(&self, expected_uid: String) -> FundingFuture<'_, FundingPreview>;
    fn replay_funding(
        &self,
        command: FundingReconciliationCommand,
    ) -> FundingFuture<'_, Option<FundingReconciliationResult>>;
}
