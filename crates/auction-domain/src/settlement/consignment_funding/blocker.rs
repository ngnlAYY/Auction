//! 现金退件的 UID 级 opening 阻断（D9 第 4.2 节）。

use uuid::Uuid;

use super::distribution::FundingMode;
use super::errors::FundingRuleError;

/// 被阻断的现金退件腿：退款 P、卖款 S、未付载体本金 1。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedLeg {
    Refund,
    Payout,
    CarrierPrincipal,
}

/// 可能已 Take、尚未完成资金消费的现金退件；最迟随现金 Take 的起写事实同事务建立。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpeningBlock {
    pub uid: String,
    pub intake_id: Uuid,
    pub leg: BlockedLeg,
    pub original_delivery_id: Uuid,
    pub amount: i64,
    pub original_mode: FundingMode,
}

impl OpeningBlock {
    /// 构造阻断记录；UID 必须非空，金额为正。
    pub fn new(
        uid: String,
        intake_id: Uuid,
        leg: BlockedLeg,
        original_delivery_id: Uuid,
        amount: i64,
        original_mode: FundingMode,
    ) -> Result<Self, FundingRuleError> {
        if uid.trim().is_empty() || uid != uid.trim() || amount <= 0 || intake_id.is_nil() {
            return Err(FundingRuleError::InvalidBlock);
        }
        Ok(Self { uid, intake_id, leg, original_delivery_id, amount, original_mode })
    }
}

/// opening 的最终写门：本 UID 存在任一未核清阻断即拒绝（含零额建界）；别的 UID 的阻断不影响本 UID。
pub fn check_opening_gate(uid: &str, blocks: &[OpeningBlock]) -> Result<(), FundingRuleError> {
    if blocks.iter().any(|block| block.uid == uid) {
        return Err(FundingRuleError::OpeningBlocked);
    }
    Ok(())
}

/// 解除阻断所需证据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockClearanceEvidence {
    pub exact_take_proven: bool,
    pub funding_consumed: bool,
    pub returned_delivery_recorded: bool,
    pub purpose_restored: bool,
    /// 载体本金腿需要的本金核销；其他腿忽略。
    pub principal_settled: bool,
    /// 独立证据确定未发生 Take，且该次可能外写的许可已同时封闭。
    pub take_disproved_and_permit_sealed: bool,
}

/// 判定能否解除。只有 Take 证明、只消费邮件、仅有处罚事实、未知、过租与手工说明都不够。
pub fn may_clear_block(leg: BlockedLeg, evidence: &BlockClearanceEvidence) -> bool {
    if evidence.take_disproved_and_permit_sealed {
        return true;
    }
    let principal_ok = leg != BlockedLeg::CarrierPrincipal || evidence.principal_settled;
    evidence.exact_take_proven
        && evidence.funding_consumed
        && evidence.returned_delivery_recorded
        && evidence.purpose_restored
        && principal_ok
}
