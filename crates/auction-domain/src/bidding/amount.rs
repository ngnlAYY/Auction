use std::fmt::{Display, Formatter};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use crate::DomainError;

/// 论坛信用金额的最小离散网格。
pub const CREDIT_GRID: i64 = 1_000;

/// 新竞价事实及新提交代理上限的绝对边界：1B Credits；历史事实与结算总额不受该上限截断。
pub const MAX_BID_AMOUNT: i64 = 1_000_000_000;

/// 拍卖金额值对象，保证金额为正数。
/// 外部普通出价、代理上限和规则默认值通过 [`BidAmount::new`] 额外保证信用网格对齐；
/// 旧版允许任意正整数起拍价，并可能由此产生非整千自动代理事实，这些已持久化金额必须
/// 通过 [`BidAmount::from_persisted_positive`] 显式进入领域层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BidAmount(i64);

impl BidAmount {
    /// 从外部输入创建经过不变量校验的金额。
    pub fn new(value: i64) -> Result<Self, DomainError> {
        if value <= 0 || value % CREDIT_GRID != 0 {
            return Err(DomainError::InvalidAmount { value });
        }

        Ok(Self(value))
    }

    /// 从旧版兼容的持久化起拍价、当前价或公开竞价事实恢复正金额。
    /// 这个入口只供可信持久化防腐层与自动竞价计算使用；普通出价、代理上限、论坛命令和
    /// 规则配置仍必须经过 [`BidAmount::new`] 或各自领域校验的信用网格约束。需要重放旧版
    /// 自动事实的 DTO 必须逐字段显式选择专用反序列化入口。
    pub fn from_persisted_positive(value: i64) -> Result<Self, DomainError> {
        if value <= 0 {
            return Err(DomainError::InvalidAmount { value });
        }
        Ok(Self(value))
    }

    pub(crate) const fn from_validated(value: i64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> i64 {
        self.0
    }

    /// 判断金额是否满足外部竞价命令使用的信用网格。
    pub const fn is_credit_grid_aligned(self) -> bool {
        self.0 % CREDIT_GRID == 0
    }

    /// 校验新外部出价或代理上限；保留通用反序列化与历史命令重放的原金额语义。
    pub const fn is_valid_bid_input(self) -> bool {
        self.is_credit_grid_aligned() && self.0 <= MAX_BID_AMOUNT
    }

    /// 自动代理使用的有效价格上限，不改写已持久化的历史授权金额。
    pub(crate) fn bid_ceiling(self) -> Self {
        Self(self.0.min(MAX_BID_AMOUNT))
    }

    /// 追加一个正增量并保留当前金额的网格余数；超过整数边界时失败关闭。
    pub(crate) fn checked_add(self, increment: i64) -> Result<Self, DomainError> {
        if increment <= 0 {
            return Err(DomainError::InvalidAmount { value: increment });
        }
        let candidate = self.0 as i128 + increment as i128;
        if candidate > i64::MAX as i128 {
            return Err(DomainError::AmountOverflow);
        }
        let value = i64::try_from(candidate).map_err(|_| DomainError::AmountOverflow)?;
        Self::from_persisted_positive(value)
    }
}

impl Display for BidAmount {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<BidAmount> for i64 {
    fn from(value: BidAmount) -> Self {
        value.value()
    }
}

impl Serialize for BidAmount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for BidAmount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BidAmountVisitor;

        impl Visitor<'_> for BidAmountVisitor {
            type Value = BidAmount;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a positive integer aligned to the credit grid")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                BidAmount::new(value).map_err(E::custom)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let value = i64::try_from(value).map_err(E::custom)?;
                BidAmount::new(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_i64(BidAmountVisitor)
    }
}

/// 仅供领域持久化 DTO 精确恢复旧版正金额，不放宽通用 [`BidAmount`] 反序列化。
pub(crate) fn deserialize_persisted_positive<'de, D>(deserializer: D) -> Result<BidAmount, D::Error>
where
    D: Deserializer<'de>,
{
    let value = i64::deserialize(deserializer)?;
    BidAmount::from_persisted_positive(value).map_err(de::Error::custom)
}

/// 仅供领域持久化 DTO 精确恢复可空旧版正金额。
pub(crate) fn deserialize_optional_persisted_positive<'de, D>(
    deserializer: D,
) -> Result<Option<BidAmount>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<i64>::deserialize(deserializer)?
        .map(BidAmount::from_persisted_positive)
        .transpose()
        .map_err(de::Error::custom)
}
