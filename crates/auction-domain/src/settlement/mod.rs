use std::fmt::{Display, Formatter};

use chrono::{DateTime, Duration, Utc};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use crate::{AuctionId, DomainError, EquipmentId, LotId, SettlementId, SubmissionId, UserId};

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

/// 允许出现零值的非负结算金额。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SettlementAmount(i64);

impl SettlementAmount {
    /// 从外部整数建立非负金额。
    pub fn new(value: i64) -> Result<Self, DomainError> {
        if value < 0 {
            return Err(DomainError::InvalidSettlementFacts("结算金额不能为负数"));
        }
        Ok(Self(value))
    }

    /// 返回零金额。
    pub const fn zero() -> Self {
        Self(0)
    }

    /// 返回底层整数。
    pub const fn value(self) -> i64 {
        self.0
    }

    /// 执行不允许绕过溢出检查的加法。
    pub fn checked_add(self, other: Self) -> Result<Self, DomainError> {
        self.0.checked_add(other.0).map(Self).ok_or(DomainError::SettlementAmountOverflow)
    }

    /// 执行不允许下溢的减法。
    pub fn checked_sub(self, other: Self) -> Result<Self, DomainError> {
        self.0
            .checked_sub(other.0)
            .filter(|value| *value >= 0)
            .map(Self)
            .ok_or(DomainError::SettlementAmountOverflow)
    }
}

impl Display for SettlementAmount {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Serialize for SettlementAmount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for SettlementAmount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SettlementAmountVisitor;

        impl Visitor<'_> for SettlementAmountVisitor {
            type Value = SettlementAmount;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a non-negative settlement amount")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                SettlementAmount::new(value).map_err(E::custom)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let value = i64::try_from(value).map_err(E::custom)?;
                SettlementAmount::new(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_i64(SettlementAmountVisitor)
    }
}

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
        } else if self.listed_price.value() == 0 || self.seller_amount != self.listed_price {
            return Err(DomainError::InvalidSettlementFacts("成交结算明细价格矩阵不一致"));
        }
        Ok(())
    }
}

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

/// 结算规划器最终要原子保存的结构化结果。
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
    ///
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
                let compensation = proportional_floor(available_pool, discount, discount_demand)?;
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
        if self.policy_version != policy.version
            || self.policy_mode != policy.mode
            || self.compensation_pool_enabled != policy.compensation_pool_enabled
            || self.closing_pool > self.opening_pool.checked_add(self.tax_total)?
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
