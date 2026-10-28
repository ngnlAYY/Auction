use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::DomainError;

/// 结算曲线使用的百万分比基数。
pub const SETTLEMENT_PPM: i64 = 1_000_000;

/// 可以写入拍卖策略快照的结算算法闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementMode {
    LegacyZeroV0,
    /// 历史算法：基础折扣由卖方让利，仅供既有冻结快照继续结算、读取与重放。
    HoldCurveV1,
    /// 卖方按成交价全额收款，买方全部优惠由补偿池承担。
    HoldCurveV2,
}

/// 结算恢复策略的闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryMode {
    RemindV1,
}

/// COD 退回次数的默认上限。
pub const DEFAULT_COD_RETURN_LIMIT: u32 = 3;
/// COD 退回次数的最大可配置上限。
pub const MAX_COD_RETURN_LIMIT: u32 = 10;

fn default_cod_return_limit() -> u32 {
    DEFAULT_COD_RETURN_LIMIT
}

/// 随拍卖策略一起冻结的恢复参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPolicy {
    pub version: u32,
    pub mode: RecoveryMode,
    pub payment_timeout: Duration,
    pub reminder_limit: u32,
    /// 同一 recovery 允许的 COD 退回次数上限；旧快照缺少该字段时按三次处理。
    #[serde(default = "default_cod_return_limit")]
    pub cod_return_limit: u32,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            mode: RecoveryMode::RemindV1,
            payment_timeout: Duration::days(7),
            reminder_limit: 1,
            cod_return_limit: DEFAULT_COD_RETURN_LIMIT,
        }
    }
}

impl RecoveryPolicy {
    /// 严格校验七天、单次提醒与可配置的 COD 退回上限。
    pub fn validate(self) -> Result<(), DomainError> {
        if self.version != 1
            || self.mode != RecoveryMode::RemindV1
            || self.payment_timeout != Duration::days(7)
            || self.reminder_limit != 1
            || !(1..=MAX_COD_RETURN_LIMIT).contains(&self.cod_return_limit)
        {
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
            version: 2,
            mode: SettlementMode::HoldCurveV2,
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
            SettlementMode::HoldCurveV1 | SettlementMode::HoldCurveV2 => {
                let expected_version = if self.mode == SettlementMode::HoldCurveV2 { 2 } else { 1 };
                if self.version != expected_version
                    || !self.compensation_pool_enabled
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

    /// 校验新拍卖模板；保留历史结构校验范围，避免改变已结算记录的读取与重放。
    /// 保留既有折扣严格小于 50% 的保守模板上限；本次移除额外优惠不顺带扩大可配置范围。
    /// v2 足额时只抵扣一份基础优惠，仍要求 COD 付款为正。
    pub fn validate_for_new_auction(&self) -> Result<(), DomainError> {
        self.validate()?;
        if self.mode == SettlementMode::HoldCurveV1 {
            return Err(DomainError::InvalidSettlementPolicy(
                "hold_curve_v1 仅供历史快照读取；新拍卖须使用卖方全额结算的 hold_curve_v2",
            ));
        }
        if self.anchors.iter().any(|anchor| anchor.adjustment_ppm <= -SETTLEMENT_PPM / 2) {
            return Err(DomainError::InvalidSettlementPolicy(
                "新拍卖结算折扣必须小于既有 50% 模板上限",
            ));
        }
        Ok(())
    }

    /// 仅供已完整校验策略的结算计算调用；保留原整数取整，不重复扫描全部锚点。
    pub(super) fn interpolate_validated(&self, hold_ppm: i64) -> Result<i64, DomainError> {
        if !(0..=SETTLEMENT_PPM).contains(&hold_ppm) {
            return Err(DomainError::InvalidSettlementFacts("持有时长比例超出范围"));
        }
        if self.mode == SettlementMode::LegacyZeroV0 {
            return Ok(0);
        }

        // 严格递增的 hold_ppm 已由 validate 保证；从第二个锚点找右端点，0 仍落在首段。
        let index = 1 + self.anchors[1..].partition_point(|anchor| anchor.hold_ppm < hold_ppm);
        let right = self
            .anchors
            .get(index)
            .ok_or(DomainError::InvalidSettlementFacts("持有时长比例没有落在结算曲线内"))?;
        if hold_ppm == right.hold_ppm {
            return Ok(right.adjustment_ppm);
        }
        let left = self.anchors[index - 1];
        let numerator = i128::from(hold_ppm - left.hold_ppm)
            * i128::from(right.adjustment_ppm - left.adjustment_ppm);
        let denominator = i128::from(right.hold_ppm - left.hold_ppm);
        let delta = divide_half_away_from_zero(numerator, denominator)?;
        let result = delta + i128::from(left.adjustment_ppm);
        i64::try_from(result).map_err(|_| DomainError::SettlementAmountOverflow)
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
