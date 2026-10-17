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
        if policy.mode == SettlementMode::HoldCurveV1 && scheduled_end_at <= scheduled_open_at {
            return Err(DomainError::InvalidSettlementFacts("结算计划时间窗口无效"));
        }

        let mut lines = Vec::with_capacity(facts.len());
        let mut discounts = Vec::with_capacity(facts.len());
        let mut tax_total = SettlementAmount::zero();
        let mut discount_demand = SettlementAmount::zero();

        for fact in facts {
            fact.validate(policy)?;
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
                discounts.push(SettlementAmount::zero());
                continue;
            };

            let hold_ppm = if policy.mode == SettlementMode::HoldCurveV1 {
                calculate_hold_ppm(
                    scheduled_open_at,
                    scheduled_end_at,
                    fact.leader_since
                        .ok_or(DomainError::InvalidSettlementFacts("获胜事实缺少领先起点"))?,
                    fact.effective_end_at,
                )?
            } else {
                0
            };
            let adjustment_ppm = policy.interpolate_adjustment(hold_ppm)?;
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
            let discount = if adjustment_ppm > 0 {
                let tax = floor_ppm(end_price, adjustment_ppm)?;
                line.tax_amount = tax;
                line.buyer_amount = end_price.checked_add(tax)?;
                tax_total = tax_total.checked_add(tax)?;
                SettlementAmount::zero()
            } else if adjustment_ppm < 0 {
                let discount = floor_ppm(end_price, -adjustment_ppm)?;
                line.listed_price = end_price.checked_sub(discount)?;
                line.buyer_amount = line.listed_price;
                line.seller_amount = line.listed_price;
                discount_demand = discount_demand.checked_add(discount)?;
                discount
            } else {
                SettlementAmount::zero()
            };
            lines.push(line);
            discounts.push(discount);
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
            for (line, discount) in lines.iter_mut().zip(discounts.iter().copied()) {
                if discount.value() == 0 {
                    continue;
                }
                // 旧版按需求比例分配后仍以该 lot 的实际折扣封顶；资金池大于总需求时不能
                // 让补偿超过折扣，否则买方金额和补偿池都会失去守恒。
                let compensation =
                    proportional_floor(available_pool, discount, discount_demand)?.min(discount);
                line.compensation_amount = compensation;
                line.buyer_amount = line.buyer_amount.checked_sub(compensation)?;
                compensation_total = compensation_total.checked_add(compensation)?;
            }
        }

        let closing_pool = if policy.compensation_pool_enabled {
            available_pool.checked_sub(compensation_total)?
        } else {
            opening_pool
        };
        for line in &lines {
            line.validate()?;
        }

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
        plan.validate(policy)?;
        Ok(plan)
    }

    /// 校验结算计划的策略快照、明细守恒、义务和资金池边界。
    pub fn validate(&self, policy: &SettlementPolicy) -> Result<(), DomainError> {
        policy.validate()?;
        let expected_closing_pool = if self.compensation_pool_enabled {
            self.opening_pool.checked_add(self.tax_total)?.checked_sub(self.compensation_total)?
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
            line.validate()?;
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
    scheduled_open_at: DateTime<Utc>,
    scheduled_end_at: DateTime<Utc>,
    leader_since: DateTime<Utc>,
    effective_end_at: DateTime<Utc>,
) -> Result<i64, DomainError> {
    if scheduled_end_at <= scheduled_open_at
        || leader_since > effective_end_at
        || leader_since.timestamp_nanos_opt().is_none()
        || effective_end_at.timestamp_nanos_opt().is_none()
    {
        return Err(DomainError::InvalidSettlementFacts("持有时长事实无效"));
    }
    let planned = timestamp_nanos(scheduled_end_at) - timestamp_nanos(scheduled_open_at);
    let held = timestamp_nanos(effective_end_at) - timestamp_nanos(leader_since);
    if planned <= 0 || held < 0 {
        return Err(DomainError::InvalidSettlementFacts("持有时长比例无效"));
    }
    if held >= planned {
        return Ok(SETTLEMENT_PPM);
    }
    let value = held * i128::from(SETTLEMENT_PPM) / planned;
    i64::try_from(value).map_err(|_| DomainError::SettlementAmountOverflow)
}

fn timestamp_nanos(value: DateTime<Utc>) -> i128 {
    i128::from(value.timestamp()) * 1_000_000_000 + i128::from(value.timestamp_subsec_nanos())
}
