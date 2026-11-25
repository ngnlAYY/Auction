//! 退件接管：按 RTS 的 K 建唯一 intake 的判定。

use auction_time::Timestamp;
use uuid::Uuid;

use super::leg::ReturnLeg;
use crate::consignment::dispatch::AttemptKey;
use crate::delivery::{OriginalMailKey, settlement_world_code};
use crate::{SettlementWorld, legacy_natural_key_uuid};

const RETURN_INTAKE_NAMESPACE: &str = "consignment.return-intake.v1";

/// 接管 ID：按 RTS 的 K 全来源唯一（`k UNIQUE`）；字节冻结。
pub fn return_intake_id(k: &OriginalMailKey) -> Uuid {
    legacy_natural_key_uuid(
        RETURN_INTAKE_NAMESPACE,
        &[
            k.authenticated_bot_uid().as_bytes(),
            settlement_world_code(k.world()).as_bytes(),
            k.external_mid().as_bytes(),
        ],
    )
}

/// 一次接管绑定的全部事实；与原信最终处置、唯一 Take work、返回事实同事务写入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnIntakeFacts {
    /// RTS 的稳定原信键。
    pub k: OriginalMailKey,
    /// 原合同。
    pub contract_id: Uuid,
    /// 原义务。
    pub obligation_id: Uuid,
    /// 原发送轮。
    pub original_round: u32,
    /// 原发送 attempt 与 fence。
    pub original_attempt: AttemptKey,
    /// 原发送的 Sent MID。
    pub sent_mid: String,
    /// 可信 RTS 的观察 ID。
    pub rts_observation_id: Uuid,
    /// RTS 发生时间。
    pub rts_returned_at: Timestamp,
    /// 应退腿。
    pub leg: ReturnLeg,
    /// 冻结的退件清单摘要。
    pub frozen_manifest_digest: [u8; 32],
    /// 授权的原会话。
    pub session_id: Uuid,
}

/// 由原发送身份得出的 RTS 来源；不明时为空，只能隔离。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtsOrigin {
    /// 原合同。
    pub contract_id: Uuid,
    /// 原义务。
    pub obligation_id: Uuid,
    /// 原发送轮。
    pub original_round: u32,
    /// 原发送 attempt 与 fence。
    pub original_attempt: AttemptKey,
    /// 原发送的认证 Bot UID。
    pub bot_uid: String,
    /// 原发送世界。
    pub world: SettlementWorld,
    /// 原发送的 Sent MID。
    pub sent_mid: String,
    /// 原发送时刻。
    pub sent_at: Timestamp,
    /// 应退腿。
    pub leg: ReturnLeg,
    /// 原发送冻结的退件清单摘要。
    pub frozen_manifest_digest: [u8; 32],
    /// 授权的原会话。
    pub session_id: Uuid,
}

/// 同一 K 已存在的接管。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingIntake {
    /// 原合同。
    pub contract_id: Uuid,
    /// 原义务。
    pub obligation_id: Uuid,
    /// 原发送轮。
    pub original_round: u32,
    /// 原发送 attempt 与 fence。
    pub original_attempt: AttemptKey,
    /// 原发送的 Sent MID。
    pub sent_mid: String,
    /// 应退腿。
    pub leg: ReturnLeg,
    /// 冻结的退件清单摘要。
    pub frozen_manifest_digest: [u8; 32],
}

/// 该 K 在原信最终处置槽里已有的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KFinalDisposition {
    /// 尚无终局。
    None,
    /// 已是 `acquire_return`。
    AcquireReturn,
    /// 已是其他或相反的处置。
    Other,
}

/// `acquire_return` 锁内输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedAcquireFacts {
    /// RTS 的稳定原信键。
    pub k: OriginalMailKey,
    /// RTS 观察 ID。
    pub rts_observation_id: Uuid,
    /// RTS 发生时间。
    pub rts_returned_at: Timestamp,
    /// 已识别寄售来源且 RTS 可信。
    pub trusted_rts: bool,
    /// 由原发送身份得出的来源；不明为空。
    pub origin: Option<RtsOrigin>,
    /// RTS 观察到的退件清单摘要；读不出为空。
    pub observed_manifest_digest: Option<[u8; 32]>,
    /// 同 K 已有接管。
    pub existing_for_k: Option<ExistingIntake>,
    /// 同 K 的终局状态。
    pub k_final: KFinalDisposition,
    /// 同轮同腿已接了另一个 K。
    pub same_leg_other_k: Option<OriginalMailKey>,
}

/// 只隔离、不接管的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireQuarantine {
    /// 不是可信的寄售 RTS。
    UntrustedRts,
    /// 原身份或轮不明。
    OriginUnknown,
    /// 来源事实互相矛盾。
    OriginContradiction,
    /// 清单摘要与冻结值不符或读不出。
    ManifestMismatch,
}

/// 进入冲突案的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcquireConflict {
    /// 同轮同一应退腿第二个不同 MID。
    SameLegSecondMid,
    /// 同 K 重复到达但事实不同。
    IntakeFactsDiffer,
    /// K 已有相反或不同的最终处置。
    OppositeDisposition,
    /// 终局槽与 intake 的存在性互相矛盾（有 intake 无终局，或有终局无 intake）。
    InconsistentFinal,
}

/// 接管判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquirePlan {
    /// 同事务建 intake、Take work、终局与返回事实。
    Acquire(Box<ReturnIntakeFacts>),
    /// 同 K 同事实重放，不新增任何行。
    Replay,
    /// 只隔离，零资产动作。
    Quarantine(AcquireQuarantine),
    /// 冲突案，零资产动作。
    Conflict(AcquireConflict),
}

/// 判定一封 RTS 是否可由寄售接管。
///
/// 保护不变量 4、6：只有事实全部一致才 `Acquire`；任何矛盾都不接管，也不静默修正。
/// 顺序：终局与 intake 自洽 → 同 K 重放 → 可信性 → 来源 → 来源自洽 → 清单 → 同腿唯一。
/// 重放比对合同、义务、轮、attempt、Sent MID、腿与冻结清单摘要；RTS 观察 ID 每次到达都可能
/// 不同，不属于“同一接管”的事实，不参与比对。
pub fn decide_acquire_return(facts: &LockedAcquireFacts) -> AcquirePlan {
    match (&facts.existing_for_k, facts.k_final) {
        (_, KFinalDisposition::Other) => {
            return AcquirePlan::Conflict(AcquireConflict::OppositeDisposition);
        }
        // 终局是 acquire_return 却没有 intake，或有 intake 却没有终局：矛盾事实，失败关闭。
        (None, KFinalDisposition::AcquireReturn) | (Some(_), KFinalDisposition::None) => {
            return AcquirePlan::Conflict(AcquireConflict::InconsistentFinal);
        }
        _ => {}
    }
    if let Some(existing) = &facts.existing_for_k {
        let same = facts.origin.as_ref().is_some_and(|origin| {
            origin.contract_id == existing.contract_id
                && origin.obligation_id == existing.obligation_id
                && origin.original_round == existing.original_round
                && origin.original_attempt == existing.original_attempt
                && origin.sent_mid == existing.sent_mid
                && origin.leg == existing.leg
                && origin.frozen_manifest_digest == existing.frozen_manifest_digest
        });
        return if same {
            AcquirePlan::Replay
        } else {
            AcquirePlan::Conflict(AcquireConflict::IntakeFactsDiffer)
        };
    }
    if !facts.trusted_rts {
        return AcquirePlan::Quarantine(AcquireQuarantine::UntrustedRts);
    }
    let Some(origin) = &facts.origin else {
        return AcquirePlan::Quarantine(AcquireQuarantine::OriginUnknown);
    };
    // RTS 自身不可能就是被退回的原信；世界、Bot 与时间先后也必须自洽。
    if origin.original_round == 0
        || origin.sent_mid == facts.k.external_mid()
        || origin.world != facts.k.world()
        || origin.bot_uid != facts.k.authenticated_bot_uid()
        || facts.rts_returned_at < origin.sent_at
    {
        return AcquirePlan::Quarantine(AcquireQuarantine::OriginContradiction);
    }
    if facts.observed_manifest_digest != Some(origin.frozen_manifest_digest) {
        return AcquirePlan::Quarantine(AcquireQuarantine::ManifestMismatch);
    }
    if facts.same_leg_other_k.as_ref().is_some_and(|other| other != &facts.k) {
        return AcquirePlan::Conflict(AcquireConflict::SameLegSecondMid);
    }
    AcquirePlan::Acquire(Box::new(ReturnIntakeFacts {
        k: facts.k.clone(),
        contract_id: origin.contract_id,
        obligation_id: origin.obligation_id,
        original_round: origin.original_round,
        original_attempt: origin.original_attempt,
        sent_mid: origin.sent_mid.clone(),
        rts_observation_id: facts.rts_observation_id,
        rts_returned_at: facts.rts_returned_at,
        leg: origin.leg,
        frozen_manifest_digest: origin.frozen_manifest_digest,
        session_id: origin.session_id,
    }))
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{
        AcquireConflict, AcquirePlan, AcquireQuarantine, AttemptKey, ExistingIntake,
        KFinalDisposition, LockedAcquireFacts, OriginalMailKey, ReturnLeg, SettlementWorld,
        decide_acquire_return,
    };

    fn key(mid: &str) -> OriginalMailKey {
        OriginalMailKey::new("12345", SettlementWorld::Main, mid).expect("规范原信键")
    }

    fn locked(k_final: KFinalDisposition, existing: Option<ExistingIntake>) -> LockedAcquireFacts {
        LockedAcquireFacts {
            k: key("600"),
            rts_observation_id: Uuid::from_u128(1),
            rts_returned_at: Timestamp::UNIX_EPOCH,
            trusted_rts: true,
            origin: None,
            observed_manifest_digest: None,
            existing_for_k: existing,
            k_final,
            same_leg_other_k: None,
        }
    }

    /// K 已有其他终局：无论有无 intake、来源是否可信，都是 `OppositeDisposition`，不接管、不隔离。
    #[test]
    fn a_k_with_another_final_disposition_conflicts() {
        let existing = ExistingIntake {
            contract_id: Uuid::from_u128(2),
            obligation_id: Uuid::from_u128(3),
            original_round: 1,
            original_attempt: AttemptKey { delivery_id: Uuid::from_u128(4), fence: 1 },
            sent_mid: "900".to_owned(),
            leg: ReturnLeg::CashRefund,
            frozen_manifest_digest: [7; 32],
        };
        for existing in [None, Some(existing)] {
            assert_eq!(
                decide_acquire_return(&locked(KFinalDisposition::Other, existing)),
                AcquirePlan::Conflict(AcquireConflict::OppositeDisposition)
            );
        }
    }

    /// 终局与 intake 的存在性互相矛盾失败关闭，没有终局的 K 才继续往下判。
    #[test]
    fn final_slot_and_intake_must_agree() {
        assert_eq!(
            decide_acquire_return(&locked(KFinalDisposition::AcquireReturn, None)),
            AcquirePlan::Conflict(AcquireConflict::InconsistentFinal)
        );
        assert_eq!(
            decide_acquire_return(&locked(KFinalDisposition::None, None)),
            AcquirePlan::Quarantine(AcquireQuarantine::OriginUnknown)
        );
    }
}
