//! Admin 损失案件用例：读案件，组装并提交人工结果、灭失证据录入与灭失确认命令。
//!
//! 保护不变量 1、6：命令 ID 必须是客户端 UUIDv4，规范摘要只在此处产生；说明经统一校验后才入摘要。

use std::sync::Arc;

use auction_domain::{CommandId, valid_text};
use uuid::Uuid;

use crate::ports::consignment::{
    AdminLossCaseRecord, ConfirmPositionLossCommand, ConsignmentError, ListingDelistReason,
    LossCaseAdmin, LossCaseOutcome, LossCaseOutcomeReceipt, LossLastKnownRef, LossProofReceipt,
    PositionLossReceipt, PositionLossStore, RecordLossCaseOutcomeCommand, RecordLossProofCommand,
    validate_listing_client_command_id, validate_listing_request_id,
};
use crate::ports::core::RequestHash;
use crate::services::consignment::recovery::encoding::{
    canonical_confirm_position_loss, canonical_record_loss_case_outcome,
    canonical_record_position_loss_proof,
};

/// Admin 追加人工结果的输入。
pub struct RecordLossOutcomeInput {
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
    /// 私有说明原文，经统一校验后入摘要。
    pub private_note: String,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// Admin 损失案件用例。
pub struct LossCaseAdminService {
    store: Arc<dyn LossCaseAdmin>,
}

impl LossCaseAdminService {
    /// 注入案件端口。
    pub fn new(store: Arc<dyn LossCaseAdmin>) -> Self {
        Self { store }
    }

    /// 按 position 读案件。
    pub async fn case(
        &self,
        position_id: Uuid,
    ) -> Result<Option<AdminLossCaseRecord>, ConsignmentError> {
        self.store.admin_loss_case(position_id).await
    }

    /// 追加人工结果；金额、付款与发行不在本命令的任何字段里。
    pub async fn record_outcome(
        &self,
        input: RecordLossOutcomeInput,
    ) -> Result<LossCaseOutcomeReceipt, ConsignmentError> {
        validate_listing_client_command_id(input.command_id)?;
        validate_listing_request_id(input.request_id.as_deref())?;
        let mut command = RecordLossCaseOutcomeCommand {
            admin_actor: input.admin_actor,
            command_id: input.command_id,
            position_id: input.position_id,
            case_id: input.case_id,
            expected_case_version: input.expected_case_version,
            outcome: input.outcome,
            basis_id: input.basis_id,
            private_note: ListingDelistReason::new(input.private_note)?,
            request_hash: RequestHash::new([0; 32]),
            request_id: input.request_id,
        };
        command.request_hash = canonical_record_loss_case_outcome(&command).request_hash();
        self.store.record_loss_case_outcome(command).await
    }
}

/// Admin 确认代管件灭失的输入。
pub struct ConfirmPositionLossInput {
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
    /// 私有说明原文，经统一校验后入摘要；只进审计，不进 DTO 与回执。
    pub reason: String,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// Admin 录入灭失证据的输入（rulings Q4）；摘要由存储在锁内推导，不在输入里。
pub struct RecordLossProofInput {
    /// 已认证 Admin 稳定引用。
    pub admin_actor: String,
    /// Admin 会话 UUIDv4。
    pub command_id: CommandId,
    /// 目标 position。
    pub position_id: Uuid,
    /// 独立证据来源（1–64 字节安全单行文本）。
    pub source_kind: String,
    /// 来源侧的观察身份（1–128 字节，与最后确定位置同一形状筛查）。
    pub observation_ref: String,
    /// 最后确定位置原文，经 [`LossLastKnownRef::new`] 校验后入摘要。
    pub last_known_ref: String,
    /// 私有说明原文，经统一校验后入摘要；只进审计，不进 DTO 与回执。
    pub private_note: String,
    /// 仅审计关联。
    pub request_id: Option<String>,
}

/// Admin 灭失证据录入与灭失确认用例；证据只追加，确认时锁内权责分流、案件与回执都由写端同一
/// 事务完成（7C-4b）。
pub struct PositionLossConfirmService {
    store: Arc<dyn PositionLossStore>,
}

impl PositionLossConfirmService {
    /// 注入灭失写端口。
    pub fn new(store: Arc<dyn PositionLossStore>) -> Self {
        Self { store }
    }

    /// 确认灭失并按 position 办结；同命令 ID 同请求返回原回执，异请求为 `command_id_conflict`。
    pub async fn confirm(
        &self,
        input: ConfirmPositionLossInput,
    ) -> Result<PositionLossReceipt, ConsignmentError> {
        validate_listing_client_command_id(input.command_id)?;
        validate_listing_request_id(input.request_id.as_deref())?;
        let mut command = ConfirmPositionLossCommand {
            admin_actor: input.admin_actor,
            command_id: input.command_id,
            position_id: input.position_id,
            expected_version: input.expected_version,
            proof_id: input.proof_id,
            reason: ListingDelistReason::new(input.reason)?,
            request_hash: RequestHash::new([0; 32]),
            request_id: input.request_id,
        };
        command.request_hash = canonical_confirm_position_loss(&command).request_hash();
        self.store.confirm_position_loss(command).await
    }

    /// 追加一条灭失证据；同命令 ID 同请求返回原回执，异请求为 `command_id_conflict`。
    ///
    /// 不变量 1、6、7：来源与观察身份在入摘要前按存储同一口径失败关闭（装备链接与 key 形状不得
    /// 进入证据或账本）；三类不足来源与推导不全由存储判 `evidence_unproven`。
    pub async fn record_proof(
        &self,
        input: RecordLossProofInput,
    ) -> Result<LossProofReceipt, ConsignmentError> {
        validate_listing_client_command_id(input.command_id)?;
        validate_listing_request_id(input.request_id.as_deref())?;
        if !valid_text(&input.source_kind, 64)
            || !valid_text(&input.observation_ref, 128)
            || LossLastKnownRef::new(input.observation_ref.clone()).is_err()
        {
            return Err(ConsignmentError::InvalidDepositRequest);
        }
        let mut command = RecordLossProofCommand {
            admin_actor: input.admin_actor,
            command_id: input.command_id,
            position_id: input.position_id,
            source_kind: input.source_kind,
            observation_ref: input.observation_ref,
            last_known_ref: LossLastKnownRef::new(input.last_known_ref)?,
            private_note: ListingDelistReason::new(input.private_note)?,
            request_hash: RequestHash::new([0; 32]),
            request_id: input.request_id,
        };
        command.request_hash = canonical_record_position_loss_proof(&command).request_hash();
        self.store.record_loss_proof(command).await
    }
}
