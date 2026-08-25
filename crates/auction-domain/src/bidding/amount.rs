use std::fmt::{Display, Formatter};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use crate::DomainError;

/// 论坛信用金额的最小离散网格。
pub const CREDIT_GRID: i64 = 1_000;

/// 拍卖金额值对象，保证金额为正数且与信用网格对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BidAmount(i64);

impl BidAmount {
    /// 最大的信用网格对齐金额；超过该边界的计算必须失败关闭。
    pub const MAX_ALIGNED: i64 = i64::MAX - i64::MAX % CREDIT_GRID;

    /// 从外部输入创建经过不变量校验的金额。
    pub fn new(value: i64) -> Result<Self, DomainError> {
        if value <= 0 || value % CREDIT_GRID != 0 {
            return Err(DomainError::InvalidAmount { value });
        }

        Ok(Self(value))
    }

    pub(crate) const fn from_validated(value: i64) -> Self {
        Self(value)
    }

    /// 返回底层整数金额。
    pub const fn value(self) -> i64 {
        self.0
    }

    /// 返回以信用网格为单位的整数数量。
    pub const fn credit_units(self) -> i64 {
        self.0 / CREDIT_GRID
    }

    /// 追加一个正增量；金额超过信用网格边界时返回算术溢出。
    pub(crate) fn checked_add(self, increment: i64) -> Result<Self, DomainError> {
        if increment <= 0 {
            return Err(DomainError::InvalidAmount { value: increment });
        }
        let candidate = self.0 as i128 + increment as i128;
        if candidate > Self::MAX_ALIGNED as i128 {
            return Err(DomainError::AmountOverflow);
        }
        let value = i64::try_from(candidate).map_err(|_| DomainError::AmountOverflow)?;
        Self::new(value)
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
