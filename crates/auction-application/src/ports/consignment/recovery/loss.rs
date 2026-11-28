//! 代管件灭失的命令、回执、写端口与卖家本人回执读端口；回执不含任何合同、付款、退款或发行字段。

use std::fmt;

use auction_domain::consignment::{ListingState, ListingVersion};
use auction_domain::{CommandId, UserId};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::ListingDelistReason;
use crate::ports::consignment::LossCaseState;
use crate::ports::consignment::deposit::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, ReceiptDisposition,
};
use crate::ports::consignment::errors::{ConsignmentError, ConsignmentFuture};
use crate::ports::core::RequestHash;

/// 灭失证据的最后确定位置：与私有说明同一安全文本规则（1–256 字节单行，拒装备链接与已知凭据形态）。
#[derive(Clone, PartialEq, Eq)]
pub struct LossLastKnownRef(String);

impl LossLastKnownRef {
    /// 校验并包装；规则与 [`ListingDelistReason::new`] 逐项相同，错误不带原文。
    pub fn new(value: String) -> Result<Self, ConsignmentError> {
        ListingDelistReason::new(value.clone())?;
        Ok(Self(value))
    }

    /// 借用文本，供规范编码与持久化。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LossLastKnownRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 与私有说明同一内容类：命令或回执被打印时不能带出该文本原文。
        formatter.write_str("[REDACTED]")
    }
}

/// Admin 录入灭失证据；只追加，不改变 position（rulings“PIW 规划第四节”Q4）。
///
/// 请求只含来源、观察身份、最后确定位置与私有说明；后续移动链、排除在途与证据摘要由存储在锁内从
/// 代管事件与派发事实推导，不信任请求输入。私有说明只进审计，不进回执与 DTO。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordLossProofCommand {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标 position。
    pub position_id: Uuid,
    /// 独立证据来源。
    pub source_kind: String,
    /// 来源侧的观察身份。
    pub observation_ref: String,
    /// 最后确定位置。
    pub last_known_ref: LossLastKnownRef,
    /// 私有说明。
    pub private_note: ListingDelistReason,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// Admin 确认代管件灭失，按 position 办结。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPositionLossCommand {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标 position。
    pub position_id: Uuid,
    /// 已提交 position 版本。
    pub expected_version: u64,
    /// 独立证据 UUID。
    pub proof_id: Uuid,
    /// 规范私有说明。
    pub reason: ListingDelistReason,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// 损失案件的人工结果种类；只读状态，无付款能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LossCaseOutcome {
    /// 复核中。
    Reviewing,
    /// 已办结。
    Closed,
}

impl LossCaseOutcome {
    /// 稳定 wire 与账本码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reviewing => "reviewing",
            Self::Closed => "closed",
        }
    }
}

/// Admin 追加损失案件的人工结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordLossCaseOutcomeCommand {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标 position。
    pub position_id: Uuid,
    /// 案件 UUID。
    pub case_id: Uuid,
    /// 已提交案件版本。
    pub expected_case_version: u64,
    /// 结果种类。
    pub outcome: LossCaseOutcome,
    /// 依据 UUID。
    pub basis_id: Uuid,
    /// 规范私有说明。
    pub private_note: ListingDelistReason,
    /// 规范摘要。
    pub request_hash: RequestHash,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// 灭失证据回执：`loss_proof_recorded`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossProofReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 固定 `record_position_loss_proof`。
    pub operation: DepositOperation,
    /// 固定 `loss_proof_recorded`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// position UUID。
    pub position_id: Uuid,
    /// 证据 UUID。
    pub proof_id: Uuid,
    /// 证据摘要。
    pub proof_digest: [u8; 32],
}

impl LossProofReceipt {
    /// 构造固定版本、操作与处置的回执。
    pub const fn new(
        command_id: CommandId,
        position_id: Uuid,
        proof_id: Uuid,
        proof_digest: [u8; 32],
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::RecordPositionLossProof,
            disposition: ReceiptDisposition::LossProofRecorded,
            command_id,
            position_id,
            proof_id,
            proof_digest,
        }
    }
}

/// 灭失确认回执：`200 position_loss_recorded`；无合同、退款或赔款字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionLossReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 固定 `confirm_position_loss`。
    pub operation: DepositOperation,
    /// 固定 `position_loss_recorded`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 挂单 UUID。
    pub listing_id: Uuid,
    /// 提交后挂单版本。
    pub listing_version: ListingVersion,
    /// 提交后挂单状态，固定 `Closed`。
    pub state: ListingState,
    /// position UUID。
    pub position_id: Uuid,
    /// 提交后 position 版本。
    pub position_version: u64,
    /// 唯一损失事实 UUID。
    pub loss_fact_id: Uuid,
    /// 损失案件 UUID。
    pub loss_case_id: Uuid,
    /// 卖家可查回执 UUID。
    pub seller_receipt_id: Uuid,
}

impl PositionLossReceipt {
    /// 构造固定版本、操作、处置与终态的回执。
    #[allow(clippy::too_many_arguments, reason = "回执字段由合同逐项冻结，拆分只会增加中间类型")]
    pub const fn new(
        command_id: CommandId,
        listing_id: Uuid,
        listing_version: ListingVersion,
        position_id: Uuid,
        position_version: u64,
        loss_fact_id: Uuid,
        loss_case_id: Uuid,
        seller_receipt_id: Uuid,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::ConfirmPositionLoss,
            disposition: ReceiptDisposition::PositionLossRecorded,
            command_id,
            listing_id,
            listing_version,
            state: ListingState::Closed,
            position_id,
            position_version,
            loss_fact_id,
            loss_case_id,
            seller_receipt_id,
        }
    }
}

/// 损失案件结果回执：`200 case_outcome_recorded`；无发行或付款字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossCaseOutcomeReceipt {
    /// 固定 v2。
    pub schema_version: u16,
    /// 固定 `record_loss_case_outcome`。
    pub operation: DepositOperation,
    /// 固定 `case_outcome_recorded`。
    pub disposition: ReceiptDisposition,
    /// 命令 UUID。
    pub command_id: CommandId,
    /// 案件 UUID。
    pub case_id: Uuid,
    /// 提交后案件版本。
    pub case_version: u64,
    /// 结果记录 UUID。
    pub outcome_id: Uuid,
    /// 卖家可查回执 UUID。
    pub seller_receipt_id: Uuid,
}

impl LossCaseOutcomeReceipt {
    /// 构造固定版本、操作与处置的回执。
    pub const fn new(
        command_id: CommandId,
        case_id: Uuid,
        case_version: u64,
        outcome_id: Uuid,
        seller_receipt_id: Uuid,
    ) -> Self {
        Self {
            schema_version: DEPOSIT_SCHEMA_VERSION,
            operation: DepositOperation::RecordLossCaseOutcome,
            disposition: ReceiptDisposition::CaseOutcomeRecorded,
            command_id,
            case_id,
            case_version,
            outcome_id,
            seller_receipt_id,
        }
    }
}

/// 灭失写端口；证据、事实、案件与回执在同一事务提交。读模型由 7C-4a / 7E-1 另立。
pub trait PositionLossStore: Send + Sync {
    /// 系统追加灭失证据，只追加。
    fn record_loss_proof(
        &self,
        command: RecordLossProofCommand,
    ) -> ConsignmentFuture<'_, LossProofReceipt>;

    /// 确认灭失并按 position 办结。
    fn confirm_position_loss(
        &self,
        command: ConfirmPositionLossCommand,
    ) -> ConsignmentFuture<'_, PositionLossReceipt>;

    /// 追加损失案件的人工结果。
    fn record_loss_case_outcome(
        &self,
        command: RecordLossCaseOutcomeCommand,
    ) -> ConsignmentFuture<'_, LossCaseOutcomeReceipt>;
}

/// 卖家本人可查的损失回执：案件编号、当前状态与最近一份回执。
///
/// 不含私有说明、证据来源与摘要、装备 key 或任何金额字段（不变量 6、7）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellerLossCaseRecord {
    /// position UUID。
    pub position_id: Uuid,
    /// 案件 UUID。
    pub loss_case_id: Uuid,
    /// 当前状态。
    pub state: LossCaseState,
    /// 最近一份回执的记录时刻。
    pub recorded_at: Timestamp,
    /// 最近一份卖家回执 UUID。
    pub receipt_id: Uuid,
}

/// 卖家本人损失回执的只读端口；不随寄售开关关闭（D9 §7：损失案查询继续）。
pub trait SellerLossCaseRead: Send + Sync {
    /// 按 position 读本人的损失回执；非本人与不存在同为 `None`，调用方统一 404。
    fn seller_loss_case(
        &self,
        seller_user_id: UserId,
        position_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<SellerLossCaseRecord>>;
}
