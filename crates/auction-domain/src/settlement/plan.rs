use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::DomainError;

use super::{
    SETTLEMENT_PPM, SettlementAmount, SettlementFact, SettlementLine, SettlementMode,
    SettlementObligation, SettlementPolicy,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementPlan {
    pub policy_version: u32,
    pub policy_mode: SettlementMode,
    pub compensation_pool_enabled: bool,
    pub opening_pool: SettlementAmount,
    pub closing_pool: SettlementAmount,
    pub tax_total: SettlementAmount,
    pub discount_demand: SettlementAmount,
    pub compensation_total: SettlementAmount,
    pub lines: Vec<SettlementLine>,
    pub obligations: Vec<SettlementObligation>,
}

impl SettlementPlan {
    /// 根据已锁定的 lot 事实计算结算明细和补偿池变化。
    /// 该计算只产生可重建的金额结果，不生成外部投递或履约状态；义务的 stable ID 和
    /// 后续 delivery 关系由应用层在同一结算事务中补齐。
    pub fn calculate(
        policy: &SettlementPolicy,
        scheduled_open_at: DateTime<Utc>,
        scheduled_end_at: DateTime<Utc>,
        opening_pool: SettlementAmount,
        facts: &[SettlementFact],
    ) -> Result<Self, DomainError> {
        policy.validate()?;
        if facts.is_empty() {
            return Err(DomainError::InvalidSettlementFacts("结算至少需要一条 lot 事实"));
        }
        if policy.mode != SettlementMode::LegacyZeroV0 && scheduled_end_at <= scheduled_open_at {
            return Err(DomainError::InvalidSettlementFacts("结算计划时间窗口无效"));
        }

        let mut lines = Vec::with_capacity(facts.len());
        let mut tax_total = SettlementAmount::zero();
        let mut discount_demand = SettlementAmount::zero();
        // 全场复用同一纳秒时窗；无效时间仍在获胜事实处按原顺序失败。
        let planned_nanos = scheduled_end_at
            .timestamp_nanos_opt()
            .zip(scheduled_open_at.timestamp_nanos_opt())
            .map(|(end, start)| i128::from(end) - i128::from(start));

        for fact in facts {
            fact.validate_shape()?;
            let Some(end_price) = fact.end_price else {
                lines.push(SettlementLine {
                    lot_id: fact.lot_id,
                    end_price: SettlementAmount::zero(),
                    listed_price: SettlementAmount::zero(),
                    buyer_amount: SettlementAmount::zero(),
                    seller_amount: SettlementAmount::zero(),
                    tax_amount: SettlementAmount::zero(),
                    compensation_amount: SettlementAmount::zero(),
                    hold_ppm: 0,
                    adjustment_ppm: 0,
                });
                continue;
            };

            let hold_ppm = if policy.mode != SettlementMode::LegacyZeroV0 {
                calculate_hold_ppm(
                    planned_nanos,
                    fact.leader_held_nanos
                        .ok_or(DomainError::InvalidSettlementFacts("获胜事实缺少累计领先时长"))?,
                )?
            } else {
                0
            };
            let adjustment_ppm = policy.interpolate_validated(hold_ppm)?;
            let mut line = SettlementLine {
                lot_id: fact.lot_id,
                end_price,
                listed_price: end_price,
                buyer_amount: end_price,
                seller_amount: end_price,
                tax_amount: SettlementAmount::zero(),
                compensation_amount: SettlementAmount::zero(),
                hold_ppm,
                adjustment_ppm,
            };
            if adjustment_ppm > 0 {
                let tax = floor_ppm(end_price, adjustment_ppm)?;
                line.tax_amount = tax;
                line.buyer_amount = end_price.checked_add(tax)?;
                tax_total = tax_total.checked_add(tax)?;
            } else if adjustment_ppm < 0 {
                let discount = floor_ppm(end_price, -adjustment_ppm)?;
                line.listed_price = end_price.checked_sub(discount)?;
                if policy.mode == SettlementMode::HoldCurveV1 {
                    line.buyer_amount = line.listed_price;
                    line.seller_amount = line.listed_price;
                }
                discount_demand = discount_demand.checked_add(discount)?;
            }
            lines.push(line);
        }

        let available_pool = if policy.compensation_pool_enabled {
            opening_pool.checked_add(tax_total)?
        } else {
            opening_pool
        };
        let mut compensation_total = SettlementAmount::zero();
        if policy.compensation_pool_enabled
            && discount_demand.value() > 0
            && available_pool.value() > 0
        {
            // 折扣已由明细中的成交价和挂牌价完整表达，无需逐 lot 的平行临时数组。
            for line in &mut lines {
                let discount = line.end_price.checked_sub(line.listed_price)?;
                if discount.value() == 0 {
                    continue;
                }
                // v2 只分配基础优惠：min(floor(A*D/ΣD),D) = floor(D*min(1,A/ΣD))。
                // 每件向下取整，尾差留池；零预算无优惠但仍结算，卖方始终全额收款。
                // v1 在卖方原让利价上分配额外抵扣，同样按 D 封顶，冻结历史公式不变。
                let compensation =
                    proportional_floor(available_pool, discount, discount_demand)?.min(discount);
                line.buyer_amount = line.buyer_amount.checked_sub(compensation)?;
                line.compensation_amount = compensation;
                compensation_total = compensation_total.checked_add(line.compensation_amount)?;
            }
        }

        let closing_pool = if policy.compensation_pool_enabled {
            available_pool.checked_sub(compensation_total)?
        } else {
            opening_pool
        };

        let plan = Self {
            policy_version: policy.version,
            policy_mode: policy.mode,
            compensation_pool_enabled: policy.compensation_pool_enabled,
            opening_pool,
            closing_pool,
            tax_total,
            discount_demand,
            compensation_total,
            lines,
            obligations: Vec::new(),
        };
        plan.validate_with_validated_policy(policy)?;
        Ok(plan)
    }

    /// 校验结算计划的策略快照、明细守恒、义务和资金池边界。
    pub fn validate(&self, policy: &SettlementPolicy) -> Result<(), DomainError> {
        policy.validate()?;
        self.validate_with_validated_policy(policy)
    }

    /// 批量计算已校验冻结曲线；结果自检复用该前置，避免再次扫描全部锚点。
    fn validate_with_validated_policy(&self, policy: &SettlementPolicy) -> Result<(), DomainError> {
        let available_pool = if self.compensation_pool_enabled {
            self.opening_pool.checked_add(self.tax_total)?
        } else {
            self.opening_pool
        };
        let expected_closing_pool = if self.compensation_pool_enabled {
            available_pool.checked_sub(self.compensation_total)?
        } else {
            self.opening_pool
        };
        if self.policy_version != policy.version
            || self.policy_mode != policy.mode
            || self.compensation_pool_enabled != policy.compensation_pool_enabled
            || self.closing_pool != expected_closing_pool
            || self.compensation_total > self.discount_demand
            || (!self.compensation_pool_enabled && self.compensation_total.value() != 0)
        {
            return Err(DomainError::InvalidSettlementFacts("结算计划策略或资金池不一致"));
        }
        for line in &self.lines {
            line.validate_for_mode(policy.mode)?;
            if policy.mode == SettlementMode::HoldCurveV2 {
                let discount = line.end_price.checked_sub(line.listed_price)?;
                let expected = if self.discount_demand.value() > 0 {
                    proportional_floor(available_pool, discount, self.discount_demand)?
                        .min(discount)
                } else {
                    SettlementAmount::zero()
                };
                if line.compensation_amount != expected {
                    return Err(DomainError::InvalidSettlementFacts(
                        "v2 结算优惠未按全场名义需求同比缩放",
                    ));
                }
            }
        }
        for obligation in &self.obligations {
            obligation.validate()?;
        }
        Ok(())
    }
}

/// 按旧系统的整数规则执行 PPM floor。
pub fn floor_ppm(amount: SettlementAmount, ppm: i64) -> Result<SettlementAmount, DomainError> {
    if !(0..=SETTLEMENT_PPM).contains(&ppm) {
        return Err(DomainError::InvalidSettlementFacts("PPM 参数超出范围"));
    }
    let value = i128::from(amount.value()) * i128::from(ppm) / i128::from(SETTLEMENT_PPM);
    let value = i64::try_from(value).map_err(|_| DomainError::SettlementAmountOverflow)?;
    SettlementAmount::new(value)
}

/// 按需求占比执行非负比例 floor。
pub fn proportional_floor(
    available: SettlementAmount,
    demand: SettlementAmount,
    total_demand: SettlementAmount,
) -> Result<SettlementAmount, DomainError> {
    if total_demand.value() <= 0 {
        return Err(DomainError::InvalidSettlementFacts("比例总需求必须为正数"));
    }
    let value = i128::from(available.value()) * i128::from(demand.value())
        / i128::from(total_demand.value());
    let value = i64::try_from(value).map_err(|_| DomainError::SettlementAmountOverflow)?;
    SettlementAmount::new(value)
}

fn calculate_hold_ppm(
    planned_nanos: Option<i128>,
    leader_held_nanos: i64,
) -> Result<i64, DomainError> {
    let planned = planned_nanos
        .filter(|_| leader_held_nanos >= 0)
        .ok_or(DomainError::InvalidSettlementFacts("累计持有时长事实无效"))?;
    if planned <= 0 {
        return Err(DomainError::InvalidSettlementFacts("持有时长比例无效"));
    }
    if i128::from(leader_held_nanos) >= planned {
        return Ok(SETTLEMENT_PPM);
    }
    let value = i128::from(leader_held_nanos) * i128::from(SETTLEMENT_PPM) / planned;
    i64::try_from(value).map_err(|_| DomainError::SettlementAmountOverflow)
}
