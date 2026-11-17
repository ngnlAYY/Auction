//! 结算退件案件的 Worker 端口：激活持久 Take、收口 Take 结果、收口补寄寄出证明与未绑定退件的
//! 只读重新核验。
//!
//! 每一步都是单个本地事务：Take 投递先以 prepared 写入再激活，外部 Take 只由投递 Worker 在租约与
//! 发送围栏之后执行（本地先提交）。Take 结果未知或失败一律转人工核对，不自动重试，也不接受
//! 操作者声明；领回证明只消费一次。

use auction_domain::SettlementReturnCaseStatus;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::body::ReturnedSettlementBody;
use crate::ports::core::ApplicationFuture;

/// Worker 推进一个案件后的安全摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnProgress {
    pub return_mail_id: Uuid,
    pub status: SettlementReturnCaseStatus,
    pub case_version: i64,
    /// 本次追加的事件种类。
    pub event_kind: &'static str,
}

/// 领取的一项只读重新核验：读取 Bot 收件箱中的原退件邮件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnRecheckClaim {
    pub return_mail_id: Uuid,
    pub world_id: String,
    pub external_mail_id: String,
    pub case_version: i64,
    pub claim_token: Uuid,
    pub lease_until: DateTime<Utc>,
}

/// 只读核验读到的退件事实；不含正文本身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReturnMailObservation {
    pub sender: String,
    pub subject: String,
    /// 系统后缀可解析时的正文事实。
    pub body: Option<ReturnedSettlementBody>,
    pub credits: i64,
    pub cod: i64,
    /// 附件外部装备 ID，严格升序。
    pub attachment_ids: Vec<String>,
    pub observed_at: DateTime<Utc>,
}

/// 只读核验的结果：邮件仍在收件箱时给出观察；已不在收件箱时只记录未找到。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementReturnRecheckOutcome {
    Observed(SettlementReturnMailObservation),
    NotFound { observed_at: DateTime<Utc> },
}

/// 结算退件 Worker 的存储端口；无到期项时返回 `None`。
pub trait SettlementReturnWorkStore: Send + Sync {
    /// 激活一项已提交的持久 Take：锁内复核案件、世界级同邮件互斥与资产后，以受限来源
    /// `settlement_return_action` 准备并激活 Take 投递。
    fn activate_due_return_take(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<SettlementReturnProgress>>;

    /// 收口一项 Take 投递已到终态的动作：精确领取证明成立时一次性记录领回事实（现金同事务返还
    /// 原资金用途），再按计划代管或授权补寄；否则转人工核对。
    fn settle_due_return_take(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<SettlementReturnProgress>>;

    /// 收口一项补寄派发已有可信寄出证明的案件。
    fn complete_due_return_resend(
        &self,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<SettlementReturnProgress>>;

    /// 领取一项待重新核验的未绑定案件。
    fn claim_due_return_recheck(
        &self,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, Option<SettlementReturnRecheckClaim>>;

    /// 写回只读核验事实：重新满足来源、资产与唯一原派发绑定时转为待决定，否则保持隔离。
    fn record_return_recheck(
        &self,
        claim: SettlementReturnRecheckClaim,
        outcome: SettlementReturnRecheckOutcome,
    ) -> ApplicationFuture<'_, SettlementReturnProgress>;
}

/// Worker 读取 Bot 收件箱中一封退件的只读端口；实现不得领取、退回或改写邮件。
pub trait SettlementReturnMailReader: Send + Sync {
    fn read_returned_mail(
        &self,
        world_id: String,
        external_mail_id: String,
    ) -> ApplicationFuture<'_, SettlementReturnRecheckOutcome>;
}
