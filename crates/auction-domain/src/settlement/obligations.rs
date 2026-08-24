use serde::{Deserialize, Serialize};

use crate::{AuctionId, DomainError, EquipmentId, LotId, SettlementId, SubmissionId, UserId};

use super::{SettlementAmount, SettlementWorld};

/// 拍卖 owner 保存的后续业务责任种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementObligationKind {
    WinnerEquipmentCod,
    WinnerPaymentCarrierCod,
    WinnerEquipmentHandoff,
    SellerPayout,
    BuyerRefund,
    UnsoldReturn,
    WithdrawnReturn,
}

/// 结算义务在交给 delivery owner 前的业务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementObligationState {
    Ready,
    Blocked,
    Contingent,
}

/// 不含 secret、lease 或发送状态的结算业务责任事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementObligation {
    pub id: SettlementId,
    pub auction_id: AuctionId,
    pub lot_id: LotId,
    pub origin_command_id: crate::CommandId,
    pub policy_version: u32,
    pub world: SettlementWorld,
    pub kind: SettlementObligationKind,
    pub state: SettlementObligationState,
    pub state_version: u64,
    pub recipient_user_id: Option<UserId>,
    pub recipient_game_identity: String,
    pub amount: SettlementAmount,
    pub carrier_credit: u32,
    pub equipment_id: EquipmentId,
    pub submission_id: Option<SubmissionId>,
    pub prerequisite_id: Option<SettlementId>,
}

impl SettlementObligation {
    /// 校验义务种类、状态、收款人和前置关系的闭集矩阵。
    pub fn validate(&self) -> Result<(), DomainError> {
        let has_user = self.recipient_user_id.is_some_and(|user_id| !user_id.is_nil());
        let has_identity = !self.recipient_game_identity.trim().is_empty()
            && self.recipient_game_identity == self.recipient_game_identity.trim();
        if self.id.is_nil()
            || self.auction_id.is_nil()
            || self.lot_id.is_nil()
            || self.origin_command_id.is_nil()
            || self.policy_version > u32::MAX / 2
            || self.state_version == 0
            || self.equipment_id.is_nil()
            || self.submission_id.is_some_and(SubmissionId::is_nil)
            || self.prerequisite_id.is_some_and(SettlementId::is_nil)
            || has_user == has_identity
        {
            return Err(DomainError::InvalidSettlementObligation("义务身份或收款人不完整"));
        }

        let prerequisite_present = self.prerequisite_id.is_some();
        match self.kind {
            SettlementObligationKind::WinnerEquipmentCod => {
                if self.world != SettlementWorld::Main
                    || self.state != SettlementObligationState::Ready
                    || !has_user
                    || self.amount.value() == 0
                    || self.carrier_credit != 0
                    || prerequisite_present
                {
                    return Err(DomainError::InvalidSettlementObligation("main COD 义务字段无效"));
                }
            }
            SettlementObligationKind::WinnerPaymentCarrierCod => {
                if self.world != SettlementWorld::Isekai
                    || self.state != SettlementObligationState::Ready
                    || !has_user
                    || self.amount.value() == 0
                    || self.carrier_credit != 1
                    || prerequisite_present
                {
                    return Err(DomainError::InvalidSettlementObligation(
                        "isekai carrier COD 义务字段无效",
                    ));
                }
            }
            SettlementObligationKind::WinnerEquipmentHandoff => {
                if self.world != SettlementWorld::Isekai
                    || !matches!(
                        self.state,
                        SettlementObligationState::Ready | SettlementObligationState::Blocked
                    )
                    || !has_user
                    || self.amount.value() != 0
                    || self.carrier_credit != 0
                    || !prerequisite_present
                {
                    return Err(DomainError::InvalidSettlementObligation(
                        "isekai handoff 义务字段无效",
                    ));
                }
            }
            SettlementObligationKind::SellerPayout => {
                if !matches!(
                    self.state,
                    SettlementObligationState::Ready | SettlementObligationState::Blocked
                ) || !has_identity
                    || self.amount.value() == 0
                    || self.carrier_credit != 0
                    || !prerequisite_present
                {
                    return Err(DomainError::InvalidSettlementObligation("卖方付款义务字段无效"));
                }
            }
            SettlementObligationKind::BuyerRefund => {
                if !matches!(
                    self.state,
                    SettlementObligationState::Ready | SettlementObligationState::Contingent
                ) || !has_user
                    || self.amount.value() == 0
                    || self.carrier_credit != 0
                    || !prerequisite_present
                {
                    return Err(DomainError::InvalidSettlementObligation("买方退款义务字段无效"));
                }
            }
            SettlementObligationKind::UnsoldReturn | SettlementObligationKind::WithdrawnReturn => {
                if self.state != SettlementObligationState::Ready
                    || !has_identity
                    || self.amount.value() != 0
                    || self.carrier_credit != 0
                    || prerequisite_present
                {
                    return Err(DomainError::InvalidSettlementObligation("流拍返还义务字段无效"));
                }
            }
        }
        Ok(())
    }
}
