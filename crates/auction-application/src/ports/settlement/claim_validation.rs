//! 结算履约 claim 的一致性规则：execution 阶段形状与 auction-owned 上下文各自成闭集。
//!
//! 存储层按既有锁序读取行并投影成本结构；本模块只做纯判定，不读数据库、不写日志。
//! 两类结论必须分开：execution 与 delivery 快照损坏时整批失败关闭并回滚领取事务；
//! 只有 auction-owned 上下文不满足时，才把该义务隔离为 `invalid_fulfillment_fact`。

use auction_domain::{SettlementObligationKind, SettlementWorld};
use uuid::Uuid;

use super::fulfillment::SettlementFulfillmentOperation;

/// 履行执行的持久化投影；字段来自已锁定行，不包含外部协议内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementClaimExecutionSnapshot {
    pub obligation_id: Uuid,
    pub execution_state: String,
    pub state_version: i64,
    pub claim_fencing: i64,
    pub retry_count: i32,
    pub dispatch_world: String,
    pub delivery_operation: String,
    pub delivery_id: Option<Uuid>,
    pub prepare_command_id: Option<Uuid>,
    pub prepare_request_digest: Option<Vec<u8>>,
    pub activate_command_id: Option<Uuid>,
    pub activate_request_digest: Option<Vec<u8>>,
    pub delivery_snapshot_id: Option<Uuid>,
}

/// execution 与投递快照的阻断项闭集；任一出现都必须让领取事务失败关闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementClaimExecutionBlocker {
    MissingObligationIdentity,
    UnknownExecutionState,
    InvalidVersionOrCounters,
    UnknownDispatchWorld,
    UnknownDeliveryOperation,
    IncompletePrepareIdentity,
    IncompleteActivationIdentity,
    ExecutionShapeMismatch,
}

impl SettlementClaimExecutionBlocker {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingObligationIdentity => "结算履约 execution 缺少 obligation identity",
            Self::UnknownExecutionState => "结算履约 execution 阶段无效",
            Self::InvalidVersionOrCounters => "结算履约 execution 版本或计数器无效",
            Self::UnknownDispatchWorld => "结算履约世界标识无效",
            Self::UnknownDeliveryOperation => "结算履约操作无效",
            Self::IncompletePrepareIdentity => "结算履约 execution prepared identity 不完整",
            Self::IncompleteActivationIdentity => "结算履约 execution activation identity 不完整",
            Self::ExecutionShapeMismatch => "结算履约 execution 阶段字段不一致",
        }
    }
}

impl SettlementClaimExecutionSnapshot {
    /// 校验 execution 身份与阶段形状；`settled` 阶段不允许出现在领取集合中。
    pub fn evaluate(&self) -> Result<(), SettlementClaimExecutionBlocker> {
        if self.obligation_id.is_nil() {
            return Err(SettlementClaimExecutionBlocker::MissingObligationIdentity);
        }
        if !matches!(self.execution_state.as_str(), "pending" | "prepared" | "enqueued") {
            return Err(SettlementClaimExecutionBlocker::UnknownExecutionState);
        }
        if self.state_version <= 0 || self.claim_fencing < 0 || self.retry_count < 0 {
            return Err(SettlementClaimExecutionBlocker::InvalidVersionOrCounters);
        }
        if SettlementWorld::from_name(&self.dispatch_world).is_none() {
            return Err(SettlementClaimExecutionBlocker::UnknownDispatchWorld);
        }
        if SettlementFulfillmentOperation::from_name(&self.delivery_operation).is_none() {
            return Err(SettlementClaimExecutionBlocker::UnknownDeliveryOperation);
        }
        let has_prepare = self.delivery_id.is_some()
            || self.prepare_command_id.is_some()
            || self.prepare_request_digest.is_some();
        if has_prepare
            && (self.delivery_id.is_none()
                || self.prepare_command_id.is_none()
                || self.prepare_request_digest.is_none())
        {
            return Err(SettlementClaimExecutionBlocker::IncompletePrepareIdentity);
        }
        let has_activation =
            self.activate_command_id.is_some() || self.activate_request_digest.is_some();
        if has_activation
            && (self.activate_command_id.is_none()
                || self.activate_request_digest.is_none()
                || !has_prepare)
        {
            return Err(SettlementClaimExecutionBlocker::IncompleteActivationIdentity);
        }
        let expected_shape = match self.execution_state.as_str() {
            "pending" => !has_prepare && !has_activation && self.delivery_snapshot_id.is_none(),
            "prepared" => has_prepare && !has_activation && self.delivery_snapshot_id.is_some(),
            "enqueued" => has_prepare && has_activation && self.delivery_snapshot_id.is_some(),
            _ => false,
        };
        if !expected_shape {
            return Err(SettlementClaimExecutionBlocker::ExecutionShapeMismatch);
        }
        Ok(())
    }
}

/// auction-owned 上下文投影；nullable 关系保留为 `Option`，由规则判定是否可隔离。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementClaimContextSnapshot {
    pub obligation_world: String,
    pub equipment_world: Option<String>,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub origin_command_id: Uuid,
    pub equipment_id: Uuid,
    pub external_equipment_id: Option<String>,
    pub equipment_name: Option<String>,
    pub recipient_user_id: Option<Uuid>,
    pub recipient_user_game_identity: Option<String>,
    pub recipient_game_identity: Option<String>,
    pub amount: i64,
    pub carrier_credit: i64,
    pub obligation_kind: String,
    pub submission_id: Option<Uuid>,
    pub submission_source_kind: Option<String>,
    pub submission_source_reference: Option<String>,
    pub submission_mailbox_key: Option<String>,
    pub submission_external_mail_id: Option<String>,
    pub submission_seller: Option<String>,
    pub prerequisite_obligation_id: Option<Uuid>,
    pub prerequisite_row_id: Option<Uuid>,
}

/// 上下文阻断项闭集；只隔离当前义务，不放大为整批失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementClaimContextBlocker {
    WorldMismatch,
    MissingIdentity,
    EquipmentFactInvalid,
    RecipientOwnerMismatch,
    AmountInvalid,
    MissingReturnSubmission,
    ReturnSubmissionSourceInvalid,
    MissingPrerequisiteRow,
    UnknownObligationKind,
}

impl SettlementClaimContextBlocker {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorldMismatch => "结算履约上下文 world 不一致",
            Self::MissingIdentity => "结算履约上下文 identity 无效",
            Self::EquipmentFactInvalid => "结算履约上下文 equipment fact 无效",
            Self::RecipientOwnerMismatch => "结算履约上下文收件人 owner 不一致",
            Self::AmountInvalid => "结算履约上下文金额字段无效",
            Self::MissingReturnSubmission => "结算履约返还上下文缺少投稿事实",
            Self::ReturnSubmissionSourceInvalid => "结算履约返还投稿来源无效",
            Self::MissingPrerequisiteRow => "结算履约上下文缺少前置义务",
            Self::UnknownObligationKind => "结算履约义务种类无效",
        }
    }
}

impl SettlementClaimContextSnapshot {
    /// 校验 world/identity/装备/收件人/金额与返还投稿来源。
    pub fn evaluate(&self) -> Result<(), SettlementClaimContextBlocker> {
        // 先判关系上下文：前置义务存在但行缺失时优先隔离，不再继续 world 判定。
        if self.prerequisite_obligation_id.is_some() && self.prerequisite_row_id.is_none() {
            return Err(SettlementClaimContextBlocker::MissingPrerequisiteRow);
        }
        if SettlementWorld::from_name(&self.obligation_world).is_none()
            || self.equipment_world.as_deref() != Some(self.obligation_world.as_str())
        {
            return Err(SettlementClaimContextBlocker::WorldMismatch);
        }
        if self.auction_id.is_nil()
            || self.lot_id.is_nil()
            || self.origin_command_id.is_nil()
            || self.equipment_id.is_nil()
        {
            return Err(SettlementClaimContextBlocker::MissingIdentity);
        }
        let external_equipment_id = self.external_equipment_id.as_deref().unwrap_or_default();
        let equipment_name = self.equipment_name.as_deref().unwrap_or_default();
        if external_equipment_id.is_empty()
            || external_equipment_id != external_equipment_id.trim()
            || equipment_name.is_empty()
            || equipment_name != equipment_name.trim()
        {
            return Err(SettlementClaimContextBlocker::EquipmentFactInvalid);
        }
        let has_user = self.recipient_user_id.is_some_and(|id| !id.is_nil());
        let has_game_identity = self
            .recipient_game_identity
            .as_deref()
            .is_some_and(|value| !value.is_empty() && value == value.trim());
        if has_user == has_game_identity
            || self.recipient_user_id.is_some_and(|id| id.is_nil())
            || self.recipient_user_id.is_some() && self.recipient_user_game_identity.is_none()
            || self.recipient_user_id.is_none() && self.recipient_user_game_identity.is_some()
        {
            return Err(SettlementClaimContextBlocker::RecipientOwnerMismatch);
        }
        if self.amount < 0 || self.carrier_credit < 0 || self.carrier_credit > i64::from(u32::MAX) {
            return Err(SettlementClaimContextBlocker::AmountInvalid);
        }
        let Some(kind) = SettlementObligationKind::from_name(&self.obligation_kind) else {
            return Err(SettlementClaimContextBlocker::UnknownObligationKind);
        };
        if !matches!(
            kind,
            SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn
        ) {
            return Ok(());
        }
        if self.submission_id.is_none()
            || self.submission_source_kind.is_none()
            || self.submission_seller.as_deref() != self.recipient_game_identity.as_deref()
            || self
                .submission_seller
                .as_deref()
                .is_none_or(|value| value.is_empty() || value != value.trim())
        {
            return Err(SettlementClaimContextBlocker::MissingReturnSubmission);
        }
        match self.submission_source_kind.as_deref() {
            Some("mail")
                if self.submission_source_reference.is_none()
                    && self.submission_mailbox_key.as_deref()
                        == Some(format!("mooglemail:{}", self.obligation_world).as_str())
                    && self
                        .submission_external_mail_id
                        .as_deref()
                        .is_some_and(|value| !value.is_empty() && value == value.trim()) => {}
            Some("admin")
                if self
                    .submission_source_reference
                    .as_deref()
                    .is_some_and(|value| !value.is_empty() && value == value.trim())
                    && self.submission_mailbox_key.is_none()
                    && self.submission_external_mail_id.is_none() => {}
            _ => return Err(SettlementClaimContextBlocker::ReturnSubmissionSourceInvalid),
        }
        if self.recipient_game_identity.as_deref().is_none_or(str::is_empty) {
            return Err(SettlementClaimContextBlocker::MissingReturnSubmission);
        }
        Ok(())
    }
}
