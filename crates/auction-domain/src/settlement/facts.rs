//! 结算事实、明细行与价格分解校验。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{DomainError, EquipmentId, LotId, SubmissionId, UserId};

use super::{SETTLEMENT_PPM, SettlementAmount, SettlementMode, SettlementPolicy};

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
    /// 最终赢家在拍卖期间的累计领先纳秒数；由追加式 bid 事实重建，缺失时结算失败关闭。
    #[serde(default)]
    pub leader_held_nanos: Option<i64>,
    pub effective_end_at: DateTime<Utc>,
    pub seller_identity: String,
    pub equipment_id: EquipmentId,
    pub submission_id: Option<SubmissionId>,
}

impl SettlementFact {
    /// 校验结算事实的获胜/流拍矩阵和外部身份边界。
    pub fn validate(&self, policy: &SettlementPolicy) -> Result<(), DomainError> {
        policy.validate()?;
        self.validate_shape()
    }

    /// 批量计算已校验冻结策略后复用事实检查，避免逐 lot 重扫同一锚点曲线。
    pub(super) fn validate_shape(&self) -> Result<(), DomainError> {
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
                    && leader_since <= self.effective_end_at
                    && self.leader_held_nanos.is_some_and(|held| held >= 0) => {}
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
    /// 实分奖池支出；v2 仅为按全场预算同比缩放后的基础优惠，不追加第二份优惠。
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
                || (self.seller_amount != self.listed_price && seller_amount != end_price)
                || seller_amount < self.listed_price.value()
                || tax_amount > 0 && compensation_amount > 0
                || tax_amount > 0 && seller_amount != end_price
                || tax_amount == 0 && seller_amount > end_price
                || compensation_amount > 0 && self.listed_price.value() >= end_price
            {
                return Err(DomainError::InvalidSettlementFacts(
                    "成交结算明细价格、税费或补偿矩阵不一致",
                ));
            }
        }
        Ok(())
    }
}

impl SettlementLine {
    /// 返回实分奖池抵扣；v1 是历史额外优惠，v2 仅为同比缩放后的基础优惠。
    /// 抵扣前价格必须配合冻结模式校验，不能将 v2 名义折扣先扣一次再抵扣总额。
    pub fn buyer_pool_credit(&self) -> Result<SettlementAmount, DomainError> {
        self.validate()?;
        Ok(self.compensation_amount)
    }

    /// 冻结模式决定卖方是否让利；旧 v1 不得借用 v2 的全额卖方矩阵通过重放校验。
    pub fn validate_for_mode(&self, mode: SettlementMode) -> Result<(), DomainError> {
        let credit = self.buyer_pool_credit()?;
        let discount = if self.adjustment_ppm < 0 {
            super::floor_ppm(self.end_price, -self.adjustment_ppm)?
        } else {
            SettlementAmount::zero()
        };
        if self.listed_price != self.end_price.checked_sub(discount)? {
            return Err(DomainError::InvalidSettlementFacts("名义列示价与冻结曲线不一致"));
        }
        if self.end_price.value() > 0 {
            validate_settlement_price_breakdown_for_mode(
                mode,
                self.end_price.value(),
                self.adjustment_ppm,
                self.buyer_amount.checked_add(credit)?.value(),
                credit.value(),
            )?;
        }
        let valid = match mode {
            SettlementMode::LegacyZeroV0 => {
                self.adjustment_ppm == 0
                    && self.listed_price == self.end_price
                    && self.seller_amount == self.end_price
                    && self.tax_amount.value() == 0
                    && self.compensation_amount.value() == 0
            }
            SettlementMode::HoldCurveV1 => {
                self.seller_amount == self.listed_price && self.compensation_amount <= discount
            }
            SettlementMode::HoldCurveV2 => {
                self.seller_amount == self.end_price
                    && (self.end_price.value() == 0 || self.buyer_amount.value() > 0)
                    && self.compensation_amount <= discount
            }
        };
        if !valid {
            return Err(DomainError::InvalidSettlementFacts(
                "冻结结算模式与卖方金额或奖池支出不一致",
            ));
        }
        Ok(())
    }
}

/// 按冻结模式校验账单，v2 从成交价抵扣实分基础优惠，不追加或重复预扣折扣。
/// 此处只校验单件边界；全场同比缩放必须由计划用首次锁内奖池重建。
pub fn validate_settlement_price_breakdown_for_mode(
    mode: SettlementMode,
    end_price: i64,
    adjustment_ppm: i64,
    adjusted_price: i64,
    pool_credit: i64,
) -> Result<(), DomainError> {
    if mode != SettlementMode::HoldCurveV2 || adjustment_ppm >= 0 {
        return validate_settlement_price_breakdown(
            end_price,
            adjustment_ppm,
            adjusted_price,
            pool_credit,
        );
    }
    if end_price <= 0
        || adjusted_price != end_price
        || pool_credit < 0
        || pool_credit >= end_price
        || adjustment_ppm < -SETTLEMENT_PPM
    {
        return Err(DomainError::InvalidSettlementFacts("v2 结算价格明细范围无效"));
    }
    let discount = i128::from(end_price) * -i128::from(adjustment_ppm) / i128::from(SETTLEMENT_PPM);
    if i128::from(pool_credit) > discount {
        return Err(DomainError::InvalidSettlementFacts("v2 实分优惠超过名义基础折扣"));
    }
    Ok(())
}

/// 校验旧模式冻结费率与买方价格明细一致；不读取现行曲线，也不改变已有结算事实。
/// 奖惩先按整数向下取整，奖池仅对折扣追加抵扣且最多抵扣一份基础折扣。
pub fn validate_settlement_price_breakdown(
    end_price: i64,
    adjustment_ppm: i64,
    adjusted_price: i64,
    pool_credit: i64,
) -> Result<(), DomainError> {
    if end_price <= 0
        || adjusted_price <= 0
        || pool_credit < 0
        || !(-SETTLEMENT_PPM..=SETTLEMENT_PPM).contains(&adjustment_ppm)
    {
        return Err(DomainError::InvalidSettlementFacts("结算价格明细范围无效"));
    }
    let end_price = i128::from(end_price);
    let magnitude = end_price * i128::from(adjustment_ppm).abs() / i128::from(SETTLEMENT_PPM);
    let expected_price =
        if adjustment_ppm < 0 { end_price - magnitude } else { end_price + magnitude };
    let maximum_pool_credit = if adjustment_ppm < 0 { magnitude } else { 0 };
    if i128::from(adjusted_price) != expected_price || i128::from(pool_credit) > maximum_pool_credit
    {
        return Err(DomainError::InvalidSettlementFacts("结算价格明细与冻结奖惩比例不一致"));
    }
    Ok(())
}
