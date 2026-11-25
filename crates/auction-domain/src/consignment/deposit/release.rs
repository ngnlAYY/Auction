//! 隔离解除的六种结果闭集与来源分类。
//!
//! [`ReleaseKind::AcquireReturn`] 只在来源是寄售时放行到退件接管写端（第 6 期）：来源、原轮、
//! 清单与同腿唯一由该写端在同一事务内按 `decide_acquire_return` 复判，这里不替它判定；
//! 两个旧拍卖 owner 的交接按 D9 勘误 E2 允许义务为空、且资产匹配不是恢复移交门槛。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::consignment::deposit::envelope::{MatchedDepositItem, ReturnManifest};
use crate::consignment::deposit::errors::DepositError;

/// 解除隔离的六种拟定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReleaseKind {
    /// 普通寄售整封接受。
    Accept,
    /// 普通寄售整封退回。
    Return,
    /// 空信归档。
    ArchiveEmpty,
    /// 寄售原轮退件接管；第 4 期保留 wire 与类型，不能成功。
    AcquireReturn,
    /// 交还原拍卖退件案件 owner。
    AuctionReturnCase,
    /// 交还原拍卖恢复 owner。
    AuctionRecoveryReturn,
}

impl ReleaseKind {
    /// 全部结果；持久化与 wire 闭集必须逐项对应。
    pub const ALL: [Self; 6] = [
        Self::Accept,
        Self::Return,
        Self::ArchiveEmpty,
        Self::AcquireReturn,
        Self::AuctionReturnCase,
        Self::AuctionRecoveryReturn,
    ];

    /// 稳定 wire 与审计码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Return => "return",
            Self::ArchiveEmpty => "archive_empty",
            Self::AcquireReturn => "acquire_return",
            Self::AuctionReturnCase => "auction_return_case",
            Self::AuctionRecoveryReturn => "auction_recovery_return",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// 旧拍卖退件案件的四种义务种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuctionReturnKind {
    /// 卖款。
    SellerPayout,
    /// 买家退款。
    BuyerRefund,
    /// 流拍返还。
    UnsoldReturn,
    /// 撤拍返还。
    WithdrawnReturn,
}

impl AuctionReturnKind {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SellerPayout => "seller_payout",
            Self::BuyerRefund => "buyer_refund",
            Self::UnsoldReturn => "unsold_return",
            Self::WithdrawnReturn => "withdrawn_return",
        }
    }
}

/// 旧拍卖恢复流程的三种义务种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuctionRecoveryKind {
    /// main 装备 COD 付款义务。
    WinnerEquipmentCod,
    /// isekai 在 main 寄出的付款载体 COD。
    WinnerPaymentCarrierCod,
    /// isekai 原世界的装备交割。
    WinnerEquipmentHandoff,
}

impl AuctionRecoveryKind {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WinnerEquipmentCod => "winner_equipment_cod",
            Self::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
            Self::WinnerEquipmentHandoff => "winner_equipment_handoff",
        }
    }
}

/// 旧拍卖退件案件证据；义务 UUID 可为空。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionReturnEvidence {
    /// 义务种类。
    pub kind: AuctionReturnKind,
    /// 主题中的原义务 UUID；本地不存在时为空，仍按 `unbound/obligation_unknown` 建案。
    pub obligation_id: Option<Uuid>,
    /// 原退件邮件 UUID（案件键）。
    pub return_mail_id: Uuid,
    /// 原退件证据 UUID。
    pub evidence_id: Uuid,
}

/// 旧拍卖恢复交接证据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionRecoveryEvidence {
    /// 义务种类。
    pub kind: AuctionRecoveryKind,
    /// 原付款或交割义务 UUID。
    pub obligation_id: Uuid,
    /// 原退件邮件 UUID。
    pub return_mail_id: Uuid,
    /// 原恢复记录 UUID。
    pub recovery_id: Uuid,
    /// 原恢复分类所验资产是否匹配；交接不以此作为门槛。
    pub asset_matches: bool,
}

/// 新观察指向的来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseSource {
    /// 普通寄售来源。
    Consignment,
    /// 旧拍卖退件案件来源。
    AuctionReturn(AuctionReturnEvidence),
    /// 旧拍卖恢复来源。
    AuctionRecovery(AuctionRecoveryEvidence),
}

impl ReleaseSource {
    /// 是否可支持 `auction_return_case`。
    pub const fn is_return_case(&self) -> bool {
        matches!(self, Self::AuctionReturn(_))
    }

    /// 是否可支持 `auction_recovery_return`。
    pub const fn is_recovery(&self) -> bool {
        matches!(self, Self::AuctionRecovery(_))
    }
}

/// `classify_release` 的锁内输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedReleaseFacts {
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 命令预期版本。
    pub expected_version: u64,
    /// 锁内隔离版本。
    pub actual_version: u64,
    /// 隔离仍活动。
    pub quarantine_active: bool,
    /// 所引新观察 UUID。
    pub observation_id: Uuid,
    /// 该观察此前未被引用；解除必须引用具体新观察。
    pub observation_is_new: bool,
    /// 新观察摘要。
    pub observation_digest: [u8; 32],
    /// 新观察与先前可靠清单实质矛盾。
    pub observation_contradicts_manifest: bool,
    /// 新观察指向的来源。
    pub source: ReleaseSource,
    /// 主题为 `consign`。
    pub subject_is_consign: bool,
    /// 发件人已认证。
    pub sender_verified: bool,
    /// 发件人在 allowlist 内。
    pub allowlisted: bool,
    /// 当前仍允许普通寄售接受。
    pub season_open_for_acceptance: bool,
    /// 整封清单可信且完整。
    pub assets_clear: bool,
    /// 已证明没有资产。
    pub archive_empty_proven: bool,
    /// 服务器允许原样 Return 且不扣款。
    pub return_safe: bool,
    /// 普通接受所需的逐件映射。
    pub matched_items: Vec<MatchedDepositItem>,
    /// 普通退回清单。
    pub return_manifest: Option<ReturnManifest>,
    /// 普通接受的最晚预约截止。
    pub accept_deadline: Option<Timestamp>,
    /// 已存在相反资产或资金动作。
    pub opposite_action_exists: bool,
    /// 同一 K 已有的解除结果；相容重放返回同一 owner。
    pub existing_release: Option<ReleaseKind>,
    /// 已存在终局处置（与解除不同）时阻止再排工作。
    pub disposition_taken: bool,
    /// 锁后数据库时刻。
    pub now: Timestamp,
}

/// 解除计划；字段形状由结果种类约束。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePlan {
    /// 结果种类。
    pub kind: ReleaseKind,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 所引新观察 UUID。
    pub observation_id: Uuid,
    /// 观察摘要。
    pub observation_digest: [u8; 32],
    /// 普通接受的逐件映射。
    pub matched_items: Vec<MatchedDepositItem>,
    /// 普通退回清单。
    pub return_manifest: Option<ReturnManifest>,
    /// 旧拍卖案件的可空义务 UUID。
    pub obligation_id: Option<Uuid>,
    /// 原退件邮件 UUID。
    pub return_mail_id: Option<Uuid>,
    /// 原退件证据 UUID。
    pub return_evidence_id: Option<Uuid>,
    /// 原恢复记录 UUID。
    pub recovery_id: Option<Uuid>,
    /// 原恢复分类资产不符；仅记录，不阻止交接。
    pub asset_mismatch: bool,
}

/// 根据锁内事实与拟定结果分类；失败时继续隔离。
///
/// 不变量 1、3、6、8：解除、终局、原 owner 接管与回执必须由同一事务提交；矛盾或相反动作
/// 不得另排资产工作。
pub fn classify_release(
    facts: &LockedReleaseFacts,
    proposed: ReleaseKind,
) -> Result<ReleasePlan, DepositError> {
    if facts.expected_version != facts.actual_version {
        return Err(DepositError::StaleVersion {
            expected: facts.expected_version,
            actual: facts.actual_version,
        });
    }
    if !facts.quarantine_active || facts.disposition_taken {
        return Err(DepositError::DispositionConflict);
    }
    if facts.existing_release.is_some_and(|existing| existing != proposed) {
        return Err(DepositError::DispositionConflict);
    }
    if facts.opposite_action_exists {
        return Err(DepositError::DispositionConflict);
    }
    if !facts.observation_is_new {
        return Err(DepositError::EvidenceUnproven);
    }

    match proposed {
        ReleaseKind::Accept => classify_accept(facts),
        ReleaseKind::Return => classify_return(facts),
        ReleaseKind::ArchiveEmpty => classify_archive_empty(facts),
        ReleaseKind::AcquireReturn => classify_acquire_return(facts),
        ReleaseKind::AuctionReturnCase => classify_auction_return(facts),
        ReleaseKind::AuctionRecoveryReturn => classify_auction_recovery(facts),
    }
}

fn classify_accept(facts: &LockedReleaseFacts) -> Result<ReleasePlan, DepositError> {
    if !matches!(facts.source, ReleaseSource::Consignment) {
        return Err(DepositError::SourceMismatch);
    }
    if facts.observation_contradicts_manifest {
        return Err(DepositError::EvidenceConflict);
    }
    if !facts.subject_is_consign
        || !facts.sender_verified
        || !facts.allowlisted
        || !facts.assets_clear
        || facts.matched_items.is_empty()
    {
        return Err(DepositError::EvidenceUnproven);
    }
    if !facts.season_open_for_acceptance {
        return Err(DepositError::SeasonClosed);
    }
    if facts.accept_deadline.is_none_or(|deadline| deadline <= facts.now) {
        return Err(DepositError::EvidenceUnproven);
    }
    Ok(plan(facts, ReleaseKind::Accept))
}

fn classify_return(facts: &LockedReleaseFacts) -> Result<ReleasePlan, DepositError> {
    if !matches!(facts.source, ReleaseSource::Consignment) {
        return Err(DepositError::SourceMismatch);
    }
    if facts.observation_contradicts_manifest {
        return Err(DepositError::EvidenceConflict);
    }
    if !facts.assets_clear || !facts.return_safe || facts.return_manifest.is_none() {
        return Err(DepositError::EvidenceUnproven);
    }
    Ok(plan(facts, ReleaseKind::Return))
}

fn classify_archive_empty(facts: &LockedReleaseFacts) -> Result<ReleasePlan, DepositError> {
    if !matches!(facts.source, ReleaseSource::Consignment) {
        return Err(DepositError::SourceMismatch);
    }
    if facts.observation_contradicts_manifest {
        return Err(DepositError::EvidenceConflict);
    }
    if !facts.archive_empty_proven || !facts.assets_clear {
        return Err(DepositError::EvidenceUnproven);
    }
    Ok(plan(facts, ReleaseKind::ArchiveEmpty))
}

fn classify_auction_return(facts: &LockedReleaseFacts) -> Result<ReleasePlan, DepositError> {
    let ReleaseSource::AuctionReturn(source) = &facts.source else {
        return Err(DepositError::SourceMismatch);
    };
    if facts.observation_contradicts_manifest {
        return Err(DepositError::EvidenceConflict);
    }
    if source.return_mail_id.is_nil() || source.evidence_id.is_nil() {
        return Err(DepositError::EvidenceUnproven);
    }
    let mut plan = plan(facts, ReleaseKind::AuctionReturnCase);
    // E2：主题里的义务 UUID 在本地不存在时仍建 `unbound/obligation_unknown` 原案。
    plan.obligation_id = source.obligation_id;
    plan.return_mail_id = Some(source.return_mail_id);
    plan.return_evidence_id = Some(source.evidence_id);
    Ok(plan)
}

/// 保护不变量 4、6：只有寄售来源才可能接管；旧拍卖来源一律来源不符，交旧 owner。
fn classify_acquire_return(facts: &LockedReleaseFacts) -> Result<ReleasePlan, DepositError> {
    if !matches!(facts.source, ReleaseSource::Consignment) {
        return Err(DepositError::SourceMismatch);
    }
    if facts.observation_contradicts_manifest {
        return Err(DepositError::EvidenceConflict);
    }
    Ok(plan(facts, ReleaseKind::AcquireReturn))
}

fn classify_auction_recovery(facts: &LockedReleaseFacts) -> Result<ReleasePlan, DepositError> {
    let ReleaseSource::AuctionRecovery(source) = &facts.source else {
        return Err(DepositError::SourceMismatch);
    };
    if facts.observation_contradicts_manifest {
        return Err(DepositError::EvidenceConflict);
    }
    if source.obligation_id.is_nil()
        || source.return_mail_id.is_nil()
        || source.recovery_id.is_nil()
    {
        return Err(DepositError::EvidenceUnproven);
    }
    let mut plan = plan(facts, ReleaseKind::AuctionRecoveryReturn);
    plan.obligation_id = Some(source.obligation_id);
    plan.return_mail_id = Some(source.return_mail_id);
    plan.recovery_id = Some(source.recovery_id);
    // E2：资产匹配不是交接门槛；原 recovery 自行记录 evidence_mismatch。
    plan.asset_mismatch = !source.asset_matches;
    Ok(plan)
}

fn plan(facts: &LockedReleaseFacts, kind: ReleaseKind) -> ReleasePlan {
    ReleasePlan {
        kind,
        quarantine_id: facts.quarantine_id,
        observation_id: facts.observation_id,
        observation_digest: facts.observation_digest,
        matched_items: facts.matched_items.clone(),
        return_manifest: facts.return_manifest.clone(),
        obligation_id: None,
        return_mail_id: None,
        return_evidence_id: None,
        recovery_id: None,
        asset_mismatch: false,
    }
}

/// 第 4 期 `acquire_return` 的固定结论说明。
pub const ACQUIRE_RETURN_PHASE4_REJECTION: &str =
    "第 4 期没有真实寄售合同或原轮，acquire_return 继续隔离";

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};
    use uuid::Uuid;

    use super::{
        AuctionRecoveryEvidence, AuctionRecoveryKind, AuctionReturnEvidence, AuctionReturnKind,
        LockedReleaseFacts, ReleaseKind, ReleaseSource, classify_release,
    };
    use crate::UserId;
    use crate::consignment::deposit::envelope::{
        MatchedDepositItem, ReturnItem, ReturnItemReason, ReturnManifest,
    };
    use crate::consignment::deposit::errors::DepositError;
    use crate::consignment::{ListingEquipmentIdentity, ListingVersion};

    const NOW: Timestamp = Timestamp::UNIX_EPOCH;

    fn base(source: ReleaseSource) -> LockedReleaseFacts {
        LockedReleaseFacts {
            quarantine_id: Uuid::from_u128(1),
            expected_version: 2,
            actual_version: 2,
            quarantine_active: true,
            observation_id: Uuid::from_u128(3),
            observation_is_new: true,
            observation_digest: [4; 32],
            observation_contradicts_manifest: false,
            source,
            subject_is_consign: true,
            sender_verified: true,
            allowlisted: true,
            season_open_for_acceptance: true,
            assets_clear: true,
            archive_empty_proven: false,
            return_safe: true,
            matched_items: vec![matched()],
            return_manifest: None,
            accept_deadline: Some(NOW + Duration::hours(1)),
            opposite_action_exists: false,
            existing_release: None,
            disposition_taken: false,
            now: NOW,
        }
    }

    fn matched() -> MatchedDepositItem {
        MatchedDepositItem {
            ordinal: 1,
            listing_id: Uuid::from_u128(31),
            listing_version: ListingVersion::new(2).expect("版本"),
            seller_user_id: UserId::from_uuid(Uuid::from_u128(32)),
            reservation_id: Uuid::from_u128(33),
            reservation_generation: 1,
            equipment: ListingEquipmentIdentity::new("main", "42").expect("装备"),
            credential_ref: None,
        }
    }

    #[test]
    fn acquire_return_passes_only_for_a_consignment_source() {
        let facts = base(ReleaseSource::Consignment);
        let plan = classify_release(&facts, ReleaseKind::AcquireReturn).expect("寄售来源放行");
        assert_eq!(plan.kind, ReleaseKind::AcquireReturn);
        let old_refund = ReleaseSource::AuctionReturn(AuctionReturnEvidence {
            kind: AuctionReturnKind::BuyerRefund,
            obligation_id: None,
            return_mail_id: Uuid::from_u128(11),
            evidence_id: Uuid::from_u128(12),
        });
        assert_eq!(
            classify_release(&base(old_refund), ReleaseKind::AcquireReturn),
            Err(DepositError::SourceMismatch)
        );
    }

    #[test]
    fn auction_return_case_allows_an_unknown_obligation() {
        let source = ReleaseSource::AuctionReturn(AuctionReturnEvidence {
            kind: AuctionReturnKind::BuyerRefund,
            obligation_id: None,
            return_mail_id: Uuid::from_u128(11),
            evidence_id: Uuid::from_u128(12),
        });
        let plan = classify_release(&base(source), ReleaseKind::AuctionReturnCase)
            .expect("未知义务仍可交还原案");
        assert_eq!(plan.kind, ReleaseKind::AuctionReturnCase);
        assert_eq!(plan.obligation_id, None);
        assert_eq!(plan.return_mail_id, Some(Uuid::from_u128(11)));
        assert_eq!(plan.return_evidence_id, Some(Uuid::from_u128(12)));
    }

    #[test]
    fn auction_recovery_allows_asset_mismatch_but_requires_known_obligation() {
        let source = ReleaseSource::AuctionRecovery(AuctionRecoveryEvidence {
            kind: AuctionRecoveryKind::WinnerEquipmentCod,
            obligation_id: Uuid::from_u128(21),
            return_mail_id: Uuid::from_u128(22),
            recovery_id: Uuid::from_u128(23),
            asset_matches: false,
        });
        let plan = classify_release(&base(source), ReleaseKind::AuctionRecoveryReturn)
            .expect("资产不符不阻止恢复交接");
        assert_eq!(plan.kind, ReleaseKind::AuctionRecoveryReturn);
        assert!(plan.asset_mismatch);
        assert_eq!(plan.obligation_id, Some(Uuid::from_u128(21)));
        assert_eq!(plan.recovery_id, Some(Uuid::from_u128(23)));
    }

    #[test]
    fn source_mismatch_is_fail_closed() {
        let recovery = ReleaseSource::AuctionRecovery(AuctionRecoveryEvidence {
            kind: AuctionRecoveryKind::WinnerEquipmentCod,
            obligation_id: Uuid::from_u128(21),
            return_mail_id: Uuid::from_u128(22),
            recovery_id: Uuid::from_u128(23),
            asset_matches: true,
        });
        assert_eq!(
            classify_release(&base(recovery.clone()), ReleaseKind::AuctionReturnCase),
            Err(DepositError::SourceMismatch)
        );
        assert_eq!(
            classify_release(&base(ReleaseSource::Consignment), ReleaseKind::AuctionRecoveryReturn),
            Err(DepositError::SourceMismatch)
        );
        assert_eq!(
            classify_release(&base(recovery), ReleaseKind::Accept),
            Err(DepositError::SourceMismatch)
        );
    }

    #[test]
    fn contradiction_conflict_and_disposition_conflict_block_release() {
        let mut contradictory = base(ReleaseSource::Consignment);
        contradictory.observation_contradicts_manifest = true;
        assert_eq!(
            classify_release(&contradictory, ReleaseKind::Return),
            Err(DepositError::EvidenceConflict)
        );
        let mut opposite = base(ReleaseSource::Consignment);
        opposite.opposite_action_exists = true;
        assert_eq!(
            classify_release(&opposite, ReleaseKind::Return),
            Err(DepositError::DispositionConflict)
        );
        let mut taken = base(ReleaseSource::Consignment);
        taken.disposition_taken = true;
        assert_eq!(
            classify_release(&taken, ReleaseKind::Return),
            Err(DepositError::DispositionConflict)
        );
        let mut existing = base(ReleaseSource::Consignment);
        existing.existing_release = Some(ReleaseKind::ArchiveEmpty);
        assert_eq!(
            classify_release(&existing, ReleaseKind::Return),
            Err(DepositError::DispositionConflict)
        );
    }

    #[test]
    fn accept_requires_live_deadline_and_reliable_new_observation() {
        let mut facts = base(ReleaseSource::Consignment);
        facts.accept_deadline = Some(NOW);
        assert_eq!(
            classify_release(&facts, ReleaseKind::Accept),
            Err(DepositError::EvidenceUnproven)
        );
        let mut replay = base(ReleaseSource::Consignment);
        replay.observation_is_new = false;
        assert_eq!(
            classify_release(&replay, ReleaseKind::Accept),
            Err(DepositError::EvidenceUnproven)
        );
    }

    #[test]
    fn accept_requires_an_open_season() {
        let mut facts = base(ReleaseSource::Consignment);
        facts.season_open_for_acceptance = false;
        assert_eq!(classify_release(&facts, ReleaseKind::Accept), Err(DepositError::SeasonClosed));
    }

    #[test]
    fn normal_accept_return_and_archive_have_explicit_preconditions() {
        let mut missing_mapping = base(ReleaseSource::Consignment);
        missing_mapping.matched_items.clear();
        assert_eq!(
            classify_release(&missing_mapping, ReleaseKind::Accept),
            Err(DepositError::EvidenceUnproven)
        );
        let facts = base(ReleaseSource::Consignment);
        assert_eq!(
            classify_release(&facts, ReleaseKind::Return),
            Err(DepositError::EvidenceUnproven)
        );
        let mut empty = facts.clone();
        empty.archive_empty_proven = true;
        assert_eq!(
            classify_release(&empty, ReleaseKind::ArchiveEmpty).map(|plan| plan.kind),
            Ok(ReleaseKind::ArchiveEmpty)
        );

        let accept = classify_release(&facts, ReleaseKind::Accept).expect("完整接受");
        assert_eq!(accept.kind, ReleaseKind::Accept);
        assert_eq!(accept.matched_items.len(), 1);

        let mut returned = base(ReleaseSource::Consignment);
        returned.return_manifest = Some(ReturnManifest {
            items: vec![ReturnItem {
                ordinal: 1,
                canonical_eid: Some("42".to_owned()),
                reason: ReturnItemReason::Unregistered,
            }],
        });
        let plan = classify_release(&returned, ReleaseKind::Return).expect("完整退回");
        assert_eq!(plan.kind, ReleaseKind::Return);
        assert_eq!(plan.return_manifest.as_ref().map(|manifest| manifest.items.len()), Some(1));
    }
}
