//! 代管 position 状态与整封精确 Take 证明的应用。
//!
//! 取得证明永远是整封的：少一项、多一项、重复 marker、同名无法唯一配对、错 fence、
//! 错正文或仅 MID 消失都不授予处分权。调用方须在同一事务内对全部 position 调用
//! [`apply_custody_proof_batch`]，任何一件失败都不得部分取得。

use uuid::Uuid;

use crate::UserId;
use crate::consignment::ListingEquipmentIdentity;
use crate::consignment::deposit::errors::DepositError;
use crate::delivery::OriginalMailKey;

/// position 的独立投影状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PositionState {
    /// 已接受，等待精确 Take。
    AwaitingTake,
    /// 已取得实物，处于 Bot 代管。
    InCustody,
    /// 已由合同承诺给买家。
    Committed,
    /// 派发在途。
    InTransit,
    /// 证明冲突或结果未知，等待核对。
    Reconcile,
    /// 已交接给合法领取人。
    HandedOver,
    /// 旧季结束，保留历史。
    SeasonEnded,
    /// 已确认损失。
    Lost,
}

impl PositionState {
    /// 全部状态；迁移与持久化闭集必须逐项对应。
    pub const ALL: [Self; 8] = [
        Self::AwaitingTake,
        Self::InCustody,
        Self::Committed,
        Self::InTransit,
        Self::Reconcile,
        Self::HandedOver,
        Self::SeasonEnded,
        Self::Lost,
    ];

    /// 稳定 wire 与数据库码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingTake => "awaiting_take",
            Self::InCustody => "in_custody",
            Self::Committed => "committed",
            Self::InTransit => "in_transit",
            Self::Reconcile => "reconcile",
            Self::HandedOver => "handed_over",
            Self::SeasonEnded => "season_ended",
            Self::Lost => "lost",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }
}

/// 验证器签收状态；未签收的形状只能核对或隔离，不授处分权。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProofValidatorStatus {
    /// 已用真实成对样本签收。
    SignedOff,
    /// 仅有测试派生件，未签收。
    Unsigned,
}

/// 原信中的一个预期附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedAttachment {
    /// 原序号。
    pub ordinal: u32,
    /// 附件显示名。
    pub name: String,
    /// 规范 EID；同名不同件时必须提供。
    pub canonical_eid: Option<String>,
}

/// 系统 `Removed attachment:` 行中的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedMarker {
    /// 附件显示名。
    pub name: String,
    /// marker 中可读到的规范 EID。
    pub canonical_eid: Option<String>,
}

/// 锁内 position 事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionFacts {
    /// position UUID。
    pub position_id: Uuid,
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 卖家。
    pub seller_user_id: UserId,
    /// 世界与规范 EID。
    pub equipment: ListingEquipmentIdentity,
    /// 当前状态。
    pub state: PositionState,
    /// 当前版本。
    pub version: u64,
    /// 原信键。
    pub mail_key: OriginalMailKey,
    /// 最终决定 UUID。
    pub decision_id: Uuid,
    /// 原工作 UUID。
    pub work_id: Uuid,
    /// 原投递 UUID。
    pub delivery_id: Uuid,
    /// 起写 fence。
    pub fence: u64,
    /// 原 attempt UUID。
    pub attempt_id: Uuid,
    /// 证据会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 写前冻结的整封清单摘要。
    pub manifest_digest: [u8; 32],
}

/// 已验证的整封 Take 观察。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMailProof {
    /// 验证器签收状态。
    pub validator_status: ProofValidatorStatus,
    /// 原信键。
    pub mail_key: OriginalMailKey,
    /// 最终决定 UUID。
    pub decision_id: Uuid,
    /// 原工作 UUID。
    pub work_id: Uuid,
    /// 原投递 UUID。
    pub delivery_id: Uuid,
    /// 原 attempt UUID。
    pub attempt_id: Uuid,
    /// 原起写 fence。
    pub fence: u64,
    /// 证据会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 写前冻结的整封清单摘要。
    pub expected_manifest_digest: [u8; 32],
    /// 写后观察到的清单摘要。
    pub observed_manifest_digest: [u8; 32],
    /// 写后身份与正文仍匹配。
    pub identity_body_matches: bool,
    /// 资产已空。
    pub assets_empty: bool,
    /// 资产不可再取。
    pub assets_unavailable: bool,
    /// Credits 方向与金额。
    pub credits: i64,
    /// COD 方向与金额。
    pub cod: i64,
    /// 回执方向与金额相符。
    pub receipt_direction_matches: bool,
    /// 写前冻结的逐项附件。
    pub expected_attachments: Vec<ExpectedAttachment>,
    /// 写后系统 marker。
    pub removed_markers: Vec<RemovedMarker>,
}

/// 单件 position 的取得结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyTransition {
    /// position UUID。
    pub position_id: Uuid,
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 卖家。
    pub seller_user_id: UserId,
    /// 世界与规范 EID。
    pub equipment: ListingEquipmentIdentity,
    /// 迁移前状态。
    pub from: PositionState,
    /// 迁移后状态。
    pub to: PositionState,
    /// 迁移前版本；调用方提交时加一。
    pub version: u64,
}

/// 应用整封证明到单个 position。
///
/// 不变量 2、3、6：只有原会话、原 fence 与逐项系统 marker 全部相等的整封观察才授予处分权。
/// 本函数只作为 [`apply_custody_proof_batch`] 的内部步骤，不对外提供逐件入口。
fn apply_custody_proof(
    position: &PositionFacts,
    proof: &ValidatedMailProof,
) -> Result<CustodyTransition, DepositError> {
    if position.state != PositionState::AwaitingTake {
        return Err(DepositError::InvalidPositionState);
    }
    if proof.validator_status != ProofValidatorStatus::SignedOff {
        return Err(DepositError::EvidenceUnproven);
    }
    if proof.mail_key != position.mail_key
        || proof.decision_id != position.decision_id
        || proof.work_id != position.work_id
        || proof.delivery_id != position.delivery_id
        || proof.attempt_id != position.attempt_id
        || proof.session_binding_digest != position.session_binding_digest
    {
        return Err(DepositError::SourceMismatch);
    }
    if proof.fence != position.fence {
        return Err(DepositError::FenceMismatch);
    }
    if proof.expected_manifest_digest != position.manifest_digest
        || proof.observed_manifest_digest != proof.expected_manifest_digest
    {
        return Err(DepositError::EvidenceUnproven);
    }
    if !proof.identity_body_matches {
        return Err(DepositError::SourceMismatch);
    }
    if !proof.assets_empty
        || !proof.assets_unavailable
        || proof.credits != 0
        || proof.cod != 0
        || !proof.receipt_direction_matches
    {
        return Err(DepositError::EvidenceUnproven);
    }
    pair_attachments(&proof.expected_attachments, &proof.removed_markers)?;
    Ok(CustodyTransition {
        position_id: position.position_id,
        listing_id: position.listing_id,
        seller_user_id: position.seller_user_id,
        equipment: position.equipment.clone(),
        from: PositionState::AwaitingTake,
        to: PositionState::InCustody,
        version: position.version,
    })
}

/// 对整封全部 position 应用同一证明；任何一件失败即整体失败。
///
/// 这是取得证明的唯一公开入口；调用方不能逐件拼出部分取得。
pub fn apply_custody_proof_batch(
    positions: &[PositionFacts],
    proof: &ValidatedMailProof,
) -> Result<Vec<CustodyTransition>, DepositError> {
    if positions.is_empty() {
        return Err(DepositError::EvidenceUnproven);
    }
    positions.iter().map(|position| apply_custody_proof(position, proof)).collect()
}

fn pair_attachments(
    expected: &[ExpectedAttachment],
    markers: &[RemovedMarker],
) -> Result<(), DepositError> {
    if expected.is_empty() {
        return Err(DepositError::EvidenceUnproven);
    }
    if markers.len() > expected.len() {
        return Err(DepositError::EvidenceConflict);
    }
    let mut used = vec![false; expected.len()];
    for marker in markers {
        let mut candidate = None;
        for (index, attachment) in expected.iter().enumerate() {
            if used[index] || attachment.name != marker.name {
                continue;
            }
            if marker
                .canonical_eid
                .as_deref()
                .is_some_and(|eid| attachment.canonical_eid.as_deref() != Some(eid))
            {
                continue;
            }
            if candidate.replace(index).is_some() {
                return Err(DepositError::EvidenceConflict);
            }
        }
        let Some(index) = candidate else {
            return Err(DepositError::EvidenceConflict);
        };
        used[index] = true;
    }
    if used.iter().all(|used| *used) { Ok(()) } else { Err(DepositError::EvidenceUnproven) }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        ExpectedAttachment, PositionFacts, PositionState, ProofValidatorStatus, RemovedMarker,
        ValidatedMailProof, apply_custody_proof, apply_custody_proof_batch,
    };
    use crate::SettlementWorld;
    use crate::UserId;
    use crate::consignment::ListingEquipmentIdentity;
    use crate::consignment::deposit::errors::DepositError;
    use crate::delivery::OriginalMailKey;

    fn key() -> OriginalMailKey {
        OriginalMailKey::new("42", SettlementWorld::Main, "100").expect("测试原信键")
    }

    fn position(index: u128, eid: &str) -> PositionFacts {
        PositionFacts {
            position_id: Uuid::from_u128(index),
            listing_id: Uuid::from_u128(100 + index),
            seller_user_id: UserId::from_uuid(Uuid::from_u128(7)),
            equipment: ListingEquipmentIdentity::new("main", eid).expect("测试装备"),
            state: PositionState::AwaitingTake,
            version: 1,
            mail_key: key(),
            decision_id: Uuid::from_u128(9),
            work_id: Uuid::from_u128(10),
            delivery_id: Uuid::from_u128(11),
            fence: 3,
            attempt_id: Uuid::from_u128(12),
            session_binding_digest: [1; 32],
            manifest_digest: [2; 32],
        }
    }

    fn proof() -> ValidatedMailProof {
        ValidatedMailProof {
            validator_status: ProofValidatorStatus::SignedOff,
            mail_key: key(),
            decision_id: Uuid::from_u128(9),
            work_id: Uuid::from_u128(10),
            delivery_id: Uuid::from_u128(11),
            attempt_id: Uuid::from_u128(12),
            fence: 3,
            session_binding_digest: [1; 32],
            expected_manifest_digest: [2; 32],
            observed_manifest_digest: [2; 32],
            identity_body_matches: true,
            assets_empty: true,
            assets_unavailable: true,
            credits: 0,
            cod: 0,
            receipt_direction_matches: true,
            expected_attachments: vec![
                ExpectedAttachment {
                    ordinal: 1,
                    name: "Sword".to_owned(),
                    canonical_eid: Some("42".to_owned()),
                },
                ExpectedAttachment {
                    ordinal: 2,
                    name: "Shield".to_owned(),
                    canonical_eid: Some("43".to_owned()),
                },
            ],
            removed_markers: vec![
                RemovedMarker { name: "Sword".to_owned(), canonical_eid: Some("42".to_owned()) },
                RemovedMarker { name: "Shield".to_owned(), canonical_eid: Some("43".to_owned()) },
            ],
        }
    }

    #[test]
    fn full_envelope_acquires_every_position_at_once() {
        let transitions =
            apply_custody_proof_batch(&[position(1, "42"), position(2, "43")], &proof())
                .expect("全封应取得");
        assert_eq!(transitions.len(), 2);
        for transition in transitions {
            assert_eq!(transition.from, PositionState::AwaitingTake);
            assert_eq!(transition.to, PositionState::InCustody);
            assert_eq!(transition.version, 1);
        }
    }

    #[test]
    fn batch_is_all_or_nothing_when_one_position_fails() {
        let mut wrong_fence = position(2, "43");
        wrong_fence.fence = 4;
        assert_eq!(
            apply_custody_proof_batch(&[position(1, "42"), wrong_fence], &proof()),
            Err(DepositError::FenceMismatch)
        );
    }

    #[test]
    fn missing_extra_and_duplicate_markers_never_grant_custody() {
        let mut missing = proof();
        missing.removed_markers.pop();
        assert_eq!(
            apply_custody_proof(&position(1, "42"), &missing),
            Err(DepositError::EvidenceUnproven)
        );
        let mut extra = proof();
        extra
            .removed_markers
            .push(RemovedMarker { name: "Ring".to_owned(), canonical_eid: Some("44".to_owned()) });
        assert_eq!(
            apply_custody_proof(&position(1, "42"), &extra),
            Err(DepositError::EvidenceConflict)
        );
        let mut duplicate = proof();
        duplicate.removed_markers[1] = duplicate.removed_markers[0].clone();
        assert_eq!(
            apply_custody_proof(&position(1, "42"), &duplicate),
            Err(DepositError::EvidenceConflict)
        );
    }

    #[test]
    fn same_name_without_unique_eid_is_a_conflict() {
        let mut proof = proof();
        proof.expected_attachments[1].name = "Sword".to_owned();
        proof.removed_markers[1].name = "Sword".to_owned();
        proof.removed_markers[0].canonical_eid = None;
        proof.removed_markers[1].canonical_eid = None;
        assert_eq!(
            apply_custody_proof(&position(1, "42"), &proof),
            Err(DepositError::EvidenceConflict)
        );
    }

    #[test]
    fn wrong_fence_body_or_mid_disappearance_is_never_proof() {
        let mut wrong_fence = proof();
        wrong_fence.fence = 4;
        assert_eq!(
            apply_custody_proof(&position(1, "42"), &wrong_fence),
            Err(DepositError::FenceMismatch)
        );
        let mut wrong_body = proof();
        wrong_body.identity_body_matches = false;
        assert_eq!(
            apply_custody_proof(&position(1, "42"), &wrong_body),
            Err(DepositError::SourceMismatch)
        );
        let mut vanished = proof();
        vanished.removed_markers.clear();
        assert_eq!(
            apply_custody_proof(&position(1, "42"), &vanished),
            Err(DepositError::EvidenceUnproven)
        );
    }

    #[test]
    fn unsigned_validator_and_non_awaiting_position_are_rejected() {
        let mut unsigned = proof();
        unsigned.validator_status = ProofValidatorStatus::Unsigned;
        assert_eq!(
            apply_custody_proof(&position(1, "42"), &unsigned),
            Err(DepositError::EvidenceUnproven)
        );
        let mut custody = position(1, "42");
        custody.state = PositionState::InCustody;
        assert_eq!(
            apply_custody_proof(&custody, &proof()),
            Err(DepositError::InvalidPositionState)
        );
    }
}
