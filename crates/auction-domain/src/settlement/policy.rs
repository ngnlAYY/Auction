use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::DomainError;

/// 结算曲线使用的百万分比基数。
pub const SETTLEMENT_PPM: i64 = 1_000_000;

/// 可以写入拍卖策略快照的结算算法闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementMode {
    LegacyZeroV0,
    HoldCurveV1,
}

/// 结算恢复策略的闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryMode {
    RemindV1,
}

/// 随拍卖策略一起冻结的恢复参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPolicy {
    pub version: u32,
    pub mode: RecoveryMode,
    pub payment_timeout: Duration,
    pub reminder_limit: u32,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            mode: RecoveryMode::RemindV1,
            payment_timeout: Duration::days(7),
            reminder_limit: 1,
        }
    }
}

impl RecoveryPolicy {
    /// 严格校验当前唯一支持的七天、单次提醒策略。
    pub fn validate(self) -> Result<(), DomainError> {
        if self != Self::default() {
            return Err(DomainError::InvalidSettlementPolicy("恢复策略版本或参数未知"));
        }
        Ok(())
    }
}

/// 持有时长曲线的一个整数锚点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdjustmentAnchor {
    pub hold_ppm: i64,
    pub adjustment_ppm: i64,
}

/// 每场拍卖冻结的结算策略快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementPolicy {
    pub version: u32,
    pub mode: SettlementMode,
    pub compensation_pool_enabled: bool,
    pub isekai_carrier_credit: u32,
    pub anchors: Vec<AdjustmentAnchor>,
}

impl Default for SettlementPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            mode: SettlementMode::HoldCurveV1,
            compensation_pool_enabled: true,
            isekai_carrier_credit: 1,
            anchors: vec![
                AdjustmentAnchor { hold_ppm: 0, adjustment_ppm: 80_000 },
                AdjustmentAnchor { hold_ppm: 50_000, adjustment_ppm: 64_000 },
                AdjustmentAnchor { hold_ppm: 100_000, adjustment_ppm: 48_000 },
                AdjustmentAnchor { hold_ppm: 150_000, adjustment_ppm: 32_000 },
                AdjustmentAnchor { hold_ppm: 200_000, adjustment_ppm: 16_000 },
                AdjustmentAnchor { hold_ppm: 250_000, adjustment_ppm: 0 },
                AdjustmentAnchor { hold_ppm: 300_000, adjustment_ppm: -4_000 },
                AdjustmentAnchor { hold_ppm: 400_000, adjustment_ppm: -12_000 },
                AdjustmentAnchor { hold_ppm: 500_000, adjustment_ppm: -20_000 },
                AdjustmentAnchor { hold_ppm: SETTLEMENT_PPM, adjustment_ppm: -20_000 },
            ],
        }
    }
}

impl SettlementPolicy {
    /// 返回迁移前拍卖使用的零调整策略。
    pub fn legacy_zero() -> Self {
        Self {
            version: 0,
            mode: SettlementMode::LegacyZeroV0,
            compensation_pool_enabled: false,
            isekai_carrier_credit: 1,
            anchors: Vec::new(),
        }
    }

    /// 校验版本、模式和锚点曲线的规范化形状。
    pub fn validate(&self) -> Result<(), DomainError> {
        match self.mode {
            SettlementMode::LegacyZeroV0 => {
                if self.version != 0
                    || self.compensation_pool_enabled
                    || self.isekai_carrier_credit != 1
                    || !self.anchors.is_empty()
                {
                    return Err(DomainError::InvalidSettlementPolicy("legacy zero 策略字段不一致"));
                }
            }
            SettlementMode::HoldCurveV1 => {
                if self.version != 1
                    || self.isekai_carrier_credit != 1
                    || self.anchors.len() < 2
                    || self.anchors.first().is_none_or(|anchor| anchor.hold_ppm != 0)
                    || self.anchors.last().is_none_or(|anchor| anchor.hold_ppm != SETTLEMENT_PPM)
                {
                    return Err(DomainError::InvalidSettlementPolicy("hold curve 策略字段不一致"));
                }
            }
        }

        for (index, anchor) in self.anchors.iter().enumerate() {
            if anchor.hold_ppm < 0
                || anchor.hold_ppm > SETTLEMENT_PPM
                || anchor.adjustment_ppm < -SETTLEMENT_PPM
                || anchor.adjustment_ppm > SETTLEMENT_PPM
                || (index > 0 && anchor.hold_ppm <= self.anchors[index - 1].hold_ppm)
            {
                return Err(DomainError::InvalidSettlementPolicy("结算锚点不是严格递增曲线"));
            }
        }
        Ok(())
    }

    /// 按旧系统的整数 half-away-from-zero 规则插值调整比例。
    pub fn interpolate_adjustment(&self, hold_ppm: i64) -> Result<i64, DomainError> {
        self.validate()?;
        if !(0..=SETTLEMENT_PPM).contains(&hold_ppm) {
            return Err(DomainError::InvalidSettlementFacts("持有时长比例超出范围"));
        }
        if self.mode == SettlementMode::LegacyZeroV0 {
            return Ok(0);
        }

        for index in 1..self.anchors.len() {
            let right = self.anchors[index];
            if hold_ppm > right.hold_ppm {
                continue;
            }
            if hold_ppm == right.hold_ppm {
                return Ok(right.adjustment_ppm);
            }
            let left = self.anchors[index - 1];
            let numerator = i128::from(hold_ppm - left.hold_ppm)
                * i128::from(right.adjustment_ppm - left.adjustment_ppm);
            let denominator = i128::from(right.hold_ppm - left.hold_ppm);
            let delta = divide_half_away_from_zero(numerator, denominator)?;
            let result = delta + i128::from(left.adjustment_ppm);
            return i64::try_from(result).map_err(|_| DomainError::SettlementAmountOverflow);
        }
        Err(DomainError::InvalidSettlementFacts("持有时长比例没有落在结算曲线内"))
    }
}

fn divide_half_away_from_zero(numerator: i128, denominator: i128) -> Result<i128, DomainError> {
    if denominator <= 0 {
        return Err(DomainError::InvalidSettlementPolicy("插值分母必须为正数"));
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder.abs() * 2 < denominator {
        return Ok(quotient);
    }
    Ok(quotient + if numerator.is_negative() { -1 } else { 1 })
}
