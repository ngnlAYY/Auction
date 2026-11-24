//! 资产外写起写许可与邮件证明应用的端口。

use auction_domain::CommandId;
use auction_domain::consignment::{HeldOffMarketReason, ListingState};
use uuid::Uuid;

use crate::ports::consignment::deposit::commands::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, ListingStateChange, ReceiptDisposition,
};
use crate::ports::consignment::errors::ConsignmentFuture;
use crate::ports::core::RequestHash;

/// 可起写的资产动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DispositionAction {
    /// 领取原信附件。
    Take,
    /// 原样退回。
    Return,
}

impl DispositionAction {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Take => "take",
            Self::Return => "return",
        }
    }
}

/// 证明种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailProofKind {
    /// 整封精确领取。
    ExactTake,
    /// 整封原样退回。
    ExactReturn,
}

impl MailProofKind {
    /// 稳定 wire 码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactTake => "exact_take",
            Self::ExactReturn => "exact_return",
        }
    }
}

/// 资产起写命令；命令 ID 由 `(decision, action, delivery, fence)` 派生。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartDispositionWriteCommand {
    /// 应用层派生的 UUIDv5。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 最终决定 UUID。
    pub final_decision_id: Uuid,
    /// 资产动作。
    pub action: DispositionAction,
    /// 投递 UUID。
    pub delivery_id: Uuid,
    /// 当前 fence。
    pub fence: u64,
    /// 原工作 UUID。
    pub work_id: Uuid,
    /// 本次 attempt UUID。
    pub attempt_id: Uuid,
    /// 当前 claim token。
    pub claim_token: Uuid,
    /// 证据会话绑定摘要。
    pub session_binding_digest: [u8; 32],
    /// 写前冻结清单摘要。
    pub expected_manifest_digest: [u8; 32],
}

/// 一次明确提交成功的起写回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// `write_started`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 最终决定 UUID。
    pub decision_id: Uuid,
    /// 原工作 UUID。
    pub work_id: Uuid,
    /// 本次 attempt UUID。
    pub attempt_id: Uuid,
    /// 原 fence。
    pub fence: u64,
}

impl StartReceipt {
    /// 构造固定版本与处置的成功回执。
    pub const fn started(
        command_id: CommandId,
        decision_id: Uuid,
        work_id: Uuid,
        attempt_id: Uuid,
        fence: u64,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::StartDispositionWrite,
            disposition: ReceiptDisposition::WriteStarted,
            command_id,
            decision_id,
            work_id,
            attempt_id,
            fence,
        }
    }
}

/// 一次性 POST 许可；只能由首次起写提交创建并以值移动一次。
///
/// 不实现 `Clone`、`Serialize` 或账本重建入口；重放、新 fence 与提交未知都只能得到
/// [`WriteStart::AlreadyCommitted`]。
///
/// 存储适配器无法构造许可：
///
/// ```compile_fail,E0624
/// let _ = auction_application::consignment::PostPermit::new(
///     uuid::Uuid::nil(),
///     uuid::Uuid::nil(),
///     1,
/// );
/// ```
#[must_use = "POST 许可必须交给实际外写执行者，重复起写不会再发许可"]
pub struct PostPermit {
    work_id: Uuid,
    delivery_id: Uuid,
    fence: u64,
}

impl PostPermit {
    /// 只由应用服务在收到首次提交结果后创建；不得从账本或回执重建。
    ///
    /// `pub(crate)` 保证存储适配器无法构造许可：端口实现只能返回纯数据
    /// [`WriteStartOutcome`]，由应用服务转换成携带许可的 [`WriteStart`]。
    pub(crate) const fn new(work_id: Uuid, delivery_id: Uuid, fence: u64) -> Self {
        Self { work_id, delivery_id, fence }
    }

    /// 许可绑定的工作。
    pub const fn work_id(&self) -> Uuid {
        self.work_id
    }

    /// 许可绑定的投递。
    pub const fn delivery_id(&self) -> Uuid {
        self.delivery_id
    }

    /// 许可绑定的 fence。
    pub const fn fence(&self) -> u64 {
        self.fence
    }
}

impl std::fmt::Debug for PostPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostPermit")
            .field("work_id", &self.work_id)
            .field("delivery_id", &self.delivery_id)
            .field("fence", &self.fence)
            .finish()
    }
}

/// 存储适配器报告的起写结果；纯数据，不含 POST 许可。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteStartOutcome {
    /// 本次事务首次提交成功。
    FirstCommitted(StartReceipt),
    /// 该工作已有成功起写；不得再次 POST。
    AlreadyCommitted(StartReceipt),
}

/// 应用服务交给执行器的起写结果；只有首次提交携带许可。
///
/// 许可由应用服务根据 [`WriteStartOutcome::FirstCommitted`] 创建，端口实现方不参与。
#[derive(Debug)]
pub enum WriteStart {
    /// 本次提交首次成功，可执行一次 POST。
    FirstCommitted {
        /// 回执。
        receipt: StartReceipt,
        /// 一次性许可。
        permit: PostPermit,
    },
    /// 该工作已有成功起写；不得再次 POST。
    AlreadyCommitted(StartReceipt),
}

/// 应用邮件证明；引用原成功起写 attempt，不用当前 fence 冒充原 fence。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyMailProofCommand {
    /// 应用层派生的 UUIDv5。
    pub command_id: CommandId,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 原起写 attempt UUID。
    pub original_attempt_id: Uuid,
    /// 证明种类。
    pub proof_kind: MailProofKind,
    /// 最终决定 UUID。
    pub final_decision_id: Uuid,
    /// 原资产动作。
    pub action: DispositionAction,
    /// 原投递 UUID。
    pub delivery_id: Uuid,
    /// 原 fence。
    pub fence: u64,
    /// 证明观察 UUID。
    pub observation_id: Uuid,
    /// 验证器版本。
    pub validator_version: String,
    /// 证明摘要。
    pub proof_digest: [u8; 32],
}

/// 证明应用的三种处置；wire 码统一由 [`ReceiptDisposition`] 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProofDisposition {
    /// 取得已证明。
    TakeProven,
    /// 退回已证明。
    ReturnProven,
    /// 证据不足或冲突，转隔离/核对。
    EvidenceQuarantined,
}

impl ProofDisposition {
    /// 稳定 wire 码；与统一回执处置闭集逐字一致。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TakeProven => "take_proven",
            Self::ReturnProven => "return_proven",
            Self::EvidenceQuarantined => "evidence_quarantined",
        }
    }
}

/// 取得后逐件应用的挂牌结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustodyListingChange {
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 提交后的挂单版本。
    pub listing_version: u64,
    /// 提交后的挂单状态。
    pub state: ListingState,
    /// 留库下架原因。
    pub held_reason: Option<HeldOffMarketReason>,
    /// position UUID。
    pub position_id: Uuid,
    /// position 提交后的状态。
    pub position_state: auction_domain::consignment::deposit::PositionState,
}

/// 证明回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 操作。
    pub operation: DepositOperation,
    /// 处置。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 最终决定 UUID。
    pub decision_id: Uuid,
    /// 证明 UUID。
    pub proof_id: Uuid,
    /// 取得后逐件变化；其他处置为空。
    pub listing_changes: Vec<ListingStateChange>,
    /// 逐件 position 与留库原因。
    pub custody_changes: Vec<CustodyListingChange>,
}

impl ProofReceipt {
    /// 构造固定版本与操作的证明回执。
    pub const fn new(
        command_id: CommandId,
        disposition: ProofDisposition,
        decision_id: Uuid,
        proof_id: Uuid,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::ApplyMailProof,
            disposition: match disposition {
                ProofDisposition::TakeProven => ReceiptDisposition::TakeProven,
                ProofDisposition::ReturnProven => ReceiptDisposition::ReturnProven,
                ProofDisposition::EvidenceQuarantined => ReceiptDisposition::EvidenceQuarantined,
            },
            command_id,
            decision_id,
            proof_id,
            listing_changes: Vec::new(),
            custody_changes: Vec::new(),
        }
    }
}

/// 资产起写与证明端口。
pub trait DispositionStore: Send + Sync {
    /// 只报告本次事务是否首次提交成功；不得构造 POST 许可。
    ///
    /// 应用服务把 [`WriteStartOutcome::FirstCommitted`] 转成携带许可的
    /// [`WriteStart::FirstCommitted`]，执行器只能经应用服务拿许可。不变量 4、5：
    /// 许可只以值移动一次，不能从账本、成功回执或新 fence 重建。
    fn start_disposition_write(
        &self,
        command: StartDispositionWriteCommand,
    ) -> ConsignmentFuture<'_, WriteStartOutcome>;

    /// 应用证明；取得全封后同提全部 position 与挂牌结果。
    ///
    /// 不变量 3、6、8：证明、position 迁移、挂牌决策与事件同事务追加。
    fn apply_mail_proof(
        &self,
        command: ApplyMailProofCommand,
    ) -> ConsignmentFuture<'_, ProofReceipt>;
}
