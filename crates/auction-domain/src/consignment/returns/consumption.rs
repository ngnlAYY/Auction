//! 退件消费：精确 Take 证明到位后只消费一次的判定。

use uuid::Uuid;

use super::intake::ReturnIntakeFacts;
use super::leg::{ConsumeEffect, ReturnLeg};
use super::take::StartedRef;
use crate::consignment::deposit::ProofValidatorStatus;
use crate::delivery::OriginalMailKey;

/// 已存在的消费记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingConsumption {
    /// 被消费的接管。
    pub intake_id: Uuid,
    /// 被消费的 K。
    pub k: OriginalMailKey,
    /// 当时的证明。
    pub proof_id: Uuid,
    /// 当时证明的摘要。
    pub proof_digest: [u8; 32],
}

/// 远端精确 Take 证明的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeProofFacts {
    /// 证明 ID。
    pub proof_id: Uuid,
    /// 证明是整封精确 Take（不是 Return 或部分）。
    pub exact_take: bool,
    /// 证据完整。
    pub validated: bool,
    /// 验证器签收状态；只有已签收的验证器出具的证明才可消费。
    pub validator_status: ProofValidatorStatus,
    /// 验证器版本。
    pub validator_version: String,
    /// 证明摘要。
    pub proof_digest: [u8; 32],
    /// 证明对应的 K。
    pub k: OriginalMailKey,
    /// 证明所指的起写 attempt 与 fence。
    pub attempt: StartedRef,
}

/// 消费命令自称的引用；必须与锁内读到的起写与证明一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumeCommandRef {
    /// 命令引用的原起写 attempt 与 fence。
    pub attempt: StartedRef,
    /// 命令引用的验证器版本。
    pub validator_version: String,
    /// 命令引用的证明摘要。
    pub proof_digest: [u8; 32],
}

/// 消费判定的锁内输入；proof 必须是锁内按命令的观察 ID 重读的证明行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumeFacts {
    /// 接管 ID。
    pub intake_id: Uuid,
    /// 接管事实。
    pub intake: ReturnIntakeFacts,
    /// 该 intake 的成功起写。
    pub started: Option<StartedRef>,
    /// 该 K / intake 已有的消费。
    pub existing_consumption: Option<ExistingConsumption>,
    /// 同轮同腿已被另一接管消费。
    pub leg_consumed_by_other: bool,
    /// 提交的证明；没有为空。
    pub proof: Option<TakeProofFacts>,
    /// 命令自称的引用。
    pub command: ConsumeCommandRef,
}

/// 证据不足、需持续核对的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileReason {
    /// 还没有证明。
    EvidenceMissing,
    /// 证明不是精确 Take 或未通过验证。
    EvidenceInsufficient,
    /// 本地没有起写事实，不能凭空消费。
    NotStarted,
}

/// 事实矛盾，失败关闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumeConflict {
    /// 已被另一接管或另一 K 消费。
    ConsumedByOther,
    /// 同轮同腿已被另一接管消费。
    LegAlreadyConsumed,
    /// 命令引用或证明的 K、attempt、fence、验证器版本、摘要与本接管的起写不符。
    ProofMismatch,
}

/// 一次消费要落的效果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumePlan {
    /// 接管。
    pub intake_id: Uuid,
    /// K。
    pub k: OriginalMailKey,
    /// 证明。
    pub proof_id: Uuid,
    /// 退回腿。
    pub leg: ReturnLeg,
    /// 资产效果。
    pub effect: ConsumeEffect,
    /// 现金腿：同事务写资金回补并解除 UID 阻断；实物腿为假。
    pub funds_cash_return: bool,
}

/// 消费判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsumeDecision {
    /// 唯一一次：同事务写证明、消费与资金回补。
    Consume(ConsumePlan),
    /// 已消费，重放回原结果，不再写任何行。
    AlreadyConsumed {
        /// 原证明。
        proof_id: Uuid,
    },
    /// 证据不足，保留 `reconcile`，零新增 POST。
    Reconcile(ReconcileReason),
    /// 事实矛盾。
    Conflict(ConsumeConflict),
}

/// 判定能否消费。
///
/// 保护不变量 4、6：消费只一次（唯一 K、唯一 intake、唯一原轮资产腿）；只有精确 Take
/// 证明、起写事实、命令引用与接管互相指认才消费，证据不足（含验证器未签收）持续核对，
/// 矛盾失败关闭。
pub fn decide_consumption(facts: &ConsumeFacts) -> ConsumeDecision {
    if let Some(existing) = &facts.existing_consumption {
        return if existing.intake_id != facts.intake_id || existing.k != facts.intake.k {
            ConsumeDecision::Conflict(ConsumeConflict::ConsumedByOther)
        } else if existing.proof_digest != facts.command.proof_digest {
            // 重放必须引用同一份证明。
            ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch)
        } else {
            ConsumeDecision::AlreadyConsumed { proof_id: existing.proof_id }
        };
    }
    if facts.leg_consumed_by_other {
        return ConsumeDecision::Conflict(ConsumeConflict::LegAlreadyConsumed);
    }
    let Some(started) = facts.started else {
        return ConsumeDecision::Reconcile(ReconcileReason::NotStarted);
    };
    // 命令自称的 attempt / fence 与实际起写不一致：矛盾，不消费。
    if facts.command.attempt != started {
        return ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch);
    }
    let Some(proof) = &facts.proof else {
        return ConsumeDecision::Reconcile(ReconcileReason::EvidenceMissing);
    };
    if proof.k != facts.intake.k
        || proof.attempt != started
        || proof.validator_version != facts.command.validator_version
        || proof.proof_digest != facts.command.proof_digest
    {
        return ConsumeDecision::Conflict(ConsumeConflict::ProofMismatch);
    }
    // 验证器未签收的证明只能核对，与取得就绪口径一致。
    if !proof.exact_take
        || !proof.validated
        || proof.validator_status != ProofValidatorStatus::SignedOff
    {
        return ConsumeDecision::Reconcile(ReconcileReason::EvidenceInsufficient);
    }
    ConsumeDecision::Consume(ConsumePlan {
        intake_id: facts.intake_id,
        k: facts.intake.k.clone(),
        proof_id: proof.proof_id,
        leg: facts.intake.leg,
        effect: facts.intake.leg.effect(),
        funds_cash_return: facts.intake.leg.is_cash(),
    })
}
