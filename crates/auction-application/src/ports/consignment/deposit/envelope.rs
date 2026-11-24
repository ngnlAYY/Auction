//! 整封决定、隔离观察、重检与解除的端口命令与回执。

use auction_domain::CommandId;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeDecision, EnvelopeObservation, LockedEnvelopeFacts, ReleaseKind,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::deposit::commands::{
    DEPOSIT_SCHEMA_VERSION, DepositActor, DepositOperation, ListingStateChange, ReceiptDisposition,
};
use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::core::RequestHash;

/// 首次整封决定。`decision` 由用例根据同一份 `facts` 计算，存储须在锁内复算并核对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecideEnvelopeCommand {
    /// 应用层派生的 UUIDv5。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 稳定原信键。
    pub mail_key: OriginalMailKey,
    /// 锁内重读后的事实。
    pub facts: LockedEnvelopeFacts,
    /// 由同一事实得到的整封决定。
    pub decision: EnvelopeDecision,
}

/// 整封决定的四种处置；wire 码统一由 [`ReceiptDisposition`] 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvelopeDisposition {
    /// 整封接受。
    Accepted,
    /// 整封退回。
    ReturnDecided,
    /// 空信归档。
    ArchivedEmpty,
    /// 持久隔离。
    Quarantined,
}

impl EnvelopeDisposition {
    /// 稳定 wire 码；与统一回执处置闭集逐字一致。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::ReturnDecided => "return_decided",
            Self::ArchivedEmpty => "archived_empty",
            Self::Quarantined => "quarantined",
        }
    }
}

/// 隔离引用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantineReference {
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 首次隔离版本。
    pub version: u64,
}

/// 整封决定回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// 处置。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 决定 UUID。
    pub decision_id: Uuid,
    /// 接受时按原序号列出的逐件 L；其他处置为空。
    pub listing_changes: Vec<ListingStateChange>,
    /// 隔离时必有的引用。
    pub quarantine: Option<QuarantineReference>,
}

impl EnvelopeReceipt {
    /// 构造固定版本与操作的成功回执。
    pub const fn new(
        command_id: CommandId,
        disposition: EnvelopeDisposition,
        decision_id: Uuid,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::DecideEnvelope,
            disposition: match disposition {
                EnvelopeDisposition::Accepted => ReceiptDisposition::Accepted,
                EnvelopeDisposition::ReturnDecided => ReceiptDisposition::ReturnDecided,
                EnvelopeDisposition::ArchivedEmpty => ReceiptDisposition::ArchivedEmpty,
                EnvelopeDisposition::Quarantined => ReceiptDisposition::Quarantined,
            },
            command_id,
            decision_id,
            listing_changes: Vec::new(),
            quarantine: None,
        }
    }

    /// 校验回执形状；存储与 DTO 测试共用。
    pub fn validate_shape(&self) -> Result<(), auction_domain::consignment::deposit::DepositError> {
        use auction_domain::consignment::deposit::DepositError;
        if self.schema_version != DEPOSIT_SCHEMA_VERSION
            || self.operation != DepositOperation::DecideEnvelope
            || self.decision_id.is_nil()
        {
            return Err(DepositError::InvalidRequest);
        }
        match self.disposition {
            ReceiptDisposition::Accepted => {
                if self.listing_changes.is_empty() || self.quarantine.is_some() {
                    return Err(DepositError::InvalidRequest);
                }
            }
            ReceiptDisposition::Quarantined => {
                if self.quarantine.is_none() || !self.listing_changes.is_empty() {
                    return Err(DepositError::InvalidRequest);
                }
            }
            ReceiptDisposition::ReturnDecided | ReceiptDisposition::ArchivedEmpty => {
                if self.quarantine.is_some() || !self.listing_changes.is_empty() {
                    return Err(DepositError::InvalidRequest);
                }
            }
            _ => return Err(DepositError::InvalidRequest),
        }
        Ok(())
    }
}

/// 追加一次只读观察；同一远端观察重放不增版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendQuarantineObservationCommand {
    /// 应用层派生的 UUIDv5。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 远端观察身份。
    pub remote_observation_id: String,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 本次观察。
    pub observation: EnvelopeObservation,
    /// 受控证据引用。
    pub evidence_ref: Option<String>,
}

/// 观察追加回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// `observation_appended`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 追加后的隔离版本；同观察重放保持原值。
    pub quarantine_version: u64,
    /// 观察 UUID。
    pub observation_id: Uuid,
    /// 观察清单摘要。
    pub digest: [u8; 32],
}

impl ObservationReceipt {
    /// 构造固定版本与处置的成功回执。
    pub const fn appended(
        command_id: CommandId,
        quarantine_id: Uuid,
        quarantine_version: u64,
        observation_id: Uuid,
        digest: [u8; 32],
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::AppendQuarantineObservation,
            disposition: ReceiptDisposition::ObservationAppended,
            command_id,
            quarantine_id,
            quarantine_version,
            observation_id,
            digest,
        }
    }
}

/// Admin 排队重检。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecheckQuarantineCommand {
    /// 玩家 Admin 会话提供的 UUIDv4。
    pub command_id: CommandId,
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 预期隔离版本。
    pub expected_version: u64,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// 重检排队回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecheckReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// `recheck_queued`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 新工作 UUID。
    pub work_id: Uuid,
}

impl RecheckReceipt {
    /// 构造固定版本与处置的成功回执。
    pub const fn queued(command_id: CommandId, quarantine_id: Uuid, work_id: Uuid) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::RecheckQuarantine,
            disposition: ReceiptDisposition::RecheckQueued,
            command_id,
            quarantine_id,
            work_id,
        }
    }
}

/// 自动复判变体额外冻结的观察摘要与来源引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoReleaseBinding {
    /// 所引观察摘要。
    pub observation_digest: [u8; 32],
    /// 锁内复判所用来源引用。
    pub source_ref: String,
}

/// Admin 或自动系统的解除命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseQuarantineCommand {
    /// Admin v4 或自动 v5。
    pub command_id: CommandId,
    /// Admin 或系统 actor。
    pub actor: DepositActor,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 预期隔离版本。
    pub expected_version: u64,
    /// 所引新观察 UUID。
    pub observation_id: Uuid,
    /// 拟定结果。
    pub proposed_kind: ReleaseKind,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
    /// 自动变体的额外绑定；Admin 命令为空。
    pub auto_binding: Option<AutoReleaseBinding>,
}

/// 解除回执；只填所选分支的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// `quarantine_released`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 隔离 UUID。
    pub quarantine_id: Uuid,
    /// 解除事实 UUID。
    pub release_id: Uuid,
    /// 最终决定 UUID。
    pub decision_id: Uuid,
    /// 最终结果。
    pub decision_kind: ReleaseKind,
    /// 普通接受的逐件 L。
    pub listing_changes: Vec<ListingStateChange>,
    /// 旧拍卖案件的可空义务 UUID；E2 允许为空。
    pub obligation_id: Option<Uuid>,
    /// 原退件邮件 UUID。
    pub return_mail_id: Option<Uuid>,
    /// 原退件证据 UUID。
    pub return_evidence_id: Option<Uuid>,
    /// 原恢复记录 UUID。
    pub recovery_id: Option<Uuid>,
    /// 恢复接管事实 UUID。
    pub recovery_handoff_id: Option<Uuid>,
    /// 第 6 期才可能产生的寄售 intake。
    pub return_intake_id: Option<Uuid>,
    /// 第 6 期才可能产生的原轮。
    pub original_round: Option<u32>,
    /// 第 6 期才可能产生的 Take 工作。
    pub take_work_id: Option<Uuid>,
    /// 第 6 期才可能产生的 Take 投递。
    pub take_delivery_id: Option<Uuid>,
}

impl ReleaseReceipt {
    /// 构造固定版本与处置、只带结果种类的成功回执。
    pub const fn released(
        command_id: CommandId,
        quarantine_id: Uuid,
        release_id: Uuid,
        decision_id: Uuid,
        decision_kind: ReleaseKind,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::ReleaseQuarantine,
            disposition: ReceiptDisposition::QuarantineReleased,
            command_id,
            quarantine_id,
            release_id,
            decision_id,
            decision_kind,
            listing_changes: Vec::new(),
            obligation_id: None,
            return_mail_id: None,
            return_evidence_id: None,
            recovery_id: None,
            recovery_handoff_id: None,
            return_intake_id: None,
            original_round: None,
            take_work_id: None,
            take_delivery_id: None,
        }
    }

    /// 校验结果分支字段；第 4 期不得产生 `acquire_return` 成功回执。
    pub fn validate_shape(&self) -> Result<(), auction_domain::consignment::deposit::DepositError> {
        use auction_domain::consignment::deposit::DepositError;
        if self.schema_version != DEPOSIT_SCHEMA_VERSION
            || self.operation != DepositOperation::ReleaseQuarantine
            || self.release_id.is_nil()
            || self.decision_id.is_nil()
        {
            return Err(DepositError::InvalidRequest);
        }
        let auction_fields_empty = self.obligation_id.is_none()
            && self.return_mail_id.is_none()
            && self.return_evidence_id.is_none()
            && self.recovery_id.is_none()
            && self.recovery_handoff_id.is_none()
            && self.return_intake_id.is_none()
            && self.original_round.is_none()
            && self.take_work_id.is_none()
            && self.take_delivery_id.is_none();
        match self.decision_kind {
            ReleaseKind::Accept => {
                if self.listing_changes.is_empty() || !auction_fields_empty {
                    return Err(DepositError::InvalidRequest);
                }
            }
            ReleaseKind::Return | ReleaseKind::ArchiveEmpty => {
                if !self.listing_changes.is_empty() || !auction_fields_empty {
                    return Err(DepositError::InvalidRequest);
                }
            }
            ReleaseKind::AuctionReturnCase => {
                if !self.listing_changes.is_empty()
                    || self.return_mail_id.is_none()
                    || self.return_evidence_id.is_none()
                    || self.recovery_id.is_some()
                    || self.recovery_handoff_id.is_some()
                    || self.return_intake_id.is_some()
                {
                    return Err(DepositError::InvalidRequest);
                }
            }
            ReleaseKind::AuctionRecoveryReturn => {
                if !self.listing_changes.is_empty()
                    || self.obligation_id.is_none()
                    || self.return_mail_id.is_none()
                    || self.recovery_id.is_none()
                    || self.recovery_handoff_id.is_none()
                    || self.return_intake_id.is_some()
                {
                    return Err(DepositError::InvalidRequest);
                }
            }
            ReleaseKind::AcquireReturn => {
                if self.return_intake_id.is_none()
                    || self.original_round.is_none()
                    || self.take_work_id.is_none()
                    || self.take_delivery_id.is_none()
                {
                    return Err(DepositError::InvalidRequest);
                }
            }
        }
        Ok(())
    }
}

/// 原信终局与隔离端口。
pub trait EnvelopeStore: Send + Sync {
    /// 首次整封决定；接受须全封一一对应，末件失败零接受。
    fn decide_envelope(
        &self,
        command: DecideEnvelopeCommand,
    ) -> ConsignmentFuture<'_, EnvelopeReceipt>;

    /// 只追加观察；相同观察重放不增版本，内容矛盾留案。
    fn append_quarantine_observation(
        &self,
        command: AppendQuarantineObservationCommand,
    ) -> ConsignmentFuture<'_, ObservationReceipt>;

    /// 排队 Admin 重检，不排资产动作。
    fn recheck_quarantine(
        &self,
        command: RecheckQuarantineCommand,
    ) -> ConsignmentFuture<'_, RecheckReceipt>;

    /// 引用具体新观察复判并原子交还原 owner。
    ///
    /// 不变量 3、4、8：解除、终局、原 owner 接管与回执同事务；冲突不得排资产动作。
    fn release_quarantine(
        &self,
        command: ReleaseQuarantineCommand,
    ) -> ConsignmentFuture<'_, ReleaseReceipt>;
}
