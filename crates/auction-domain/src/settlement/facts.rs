use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{DomainError, EquipmentId, LotId, SubmissionId, UserId};

use super::{SETTLEMENT_PPM, SettlementAmount, SettlementPolicy};

/// 结算业务使用的世界闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementWorld {
    Main,
    Isekai,
}

/// 结算计算时从锁内读取的不可变 lot 事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementFact {
    pub lot_id: LotId,
    pub world: SettlementWorld,
    pub end_price: Option<SettlementAmount>,
    pub leader_id: Option<UserId>,
    pub leader_since: Option<DateTime<Utc>>,
    pub effective_end_at: DateTime<Utc>,
    pub seller_identity: String,
    pub equipment_id: EquipmentId,
    pub submission_id: Option<SubmissionId>,
}

impl SettlementFact {
    /// 校验结算事实的获胜/流拍矩阵和外部身份边界。
    pub fn validate(&self, policy: &SettlementPolicy) -> Result<(), DomainError> {
        policy.validate()?;
        if self.lot_id.is_nil()
            || self.equipment_id.is_nil()
            || self.seller_identity.trim().is_empty()
            || self.seller_identity != self.seller_identity.trim()
            || self.effective_end_at.timestamp_nanos_opt().is_none()
            || self.submission_id.is_some_and(SubmissionId::is_nil)
        {
            return Err(DomainError::InvalidSettlementFacts("结算事实身份或时间无效"));
        }

        match (self.end_price, self.leader_id, self.leader_since) {
            (Some(price), Some(leader_id), Some(leader_since))
                if price.value() > 0
                    && !leader_id.is_nil()
                    && leader_since.timestamp_nanos_opt().is_some()
                    && leader_since <= self.effective_end_at => {}
            (None, None, None) => {}
            _ => {
                return Err(DomainError::InvalidSettlementFacts(
                    "获胜结算事实必须同时包含价格、领先者和领先起点",
                ));
            }
        }
        Ok(())
    }
}

/// 结算明细中的买方、卖方、税费和补偿金额。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementLine {
    pub lot_id: LotId,
    pub end_price: SettlementAmount,
    pub listed_price: SettlementAmount,
    pub buyer_amount: SettlementAmount,
    pub seller_amount: SettlementAmount,
    pub tax_amount: SettlementAmount,
    pub compensation_amount: SettlementAmount,
    pub hold_ppm: i64,
    pub adjustment_ppm: i64,
}

impl SettlementLine {
    /// 校验金额守恒：买方支付加补偿必须等于卖方收入加税费。
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.lot_id.is_nil()
            || !(0..=SETTLEMENT_PPM).contains(&self.hold_ppm)
            || !(-SETTLEMENT_PPM..=SETTLEMENT_PPM).contains(&self.adjustment_ppm)
        {
            return Err(DomainError::InvalidSettlementFacts("结算明细元数据无效"));
        }
        let left =
            i128::from(self.buyer_amount.value()) + i128::from(self.compensation_amount.value());
        let right = i128::from(self.seller_amount.value()) + i128::from(self.tax_amount.value());
        if left != right {
            return Err(DomainError::InvalidSettlementFacts("结算明细金额不守恒"));
        }
        if self.end_price.value() == 0 {
            if self.listed_price.value() != 0
                || self.buyer_amount.value() != 0
                || self.seller_amount.value() != 0
                || self.tax_amount.value() != 0
                || self.compensation_amount.value() != 0
            {
                return Err(DomainError::InvalidSettlementFacts("流拍结算明细金额不一致"));
            }
        } else {
            let end_price = self.end_price.value();
            let seller_amount = self.seller_amount.value();
            let tax_amount = self.tax_amount.value();
            let compensation_amount = self.compensation_amount.value();
            if self.listed_price.value() == 0
                || self.seller_amount != self.listed_price
                || tax_amount > 0 && compensation_amount > 0
                || tax_amount > 0 && seller_amount != end_price
                || tax_amount == 0 && seller_amount > end_price
                || compensation_amount > 0 && seller_amount >= end_price
            {
                return Err(DomainError::InvalidSettlementFacts(
                    "成交结算明细价格、税费或补偿矩阵不一致",
                ));
            }
        }
        Ok(())
    }
}
