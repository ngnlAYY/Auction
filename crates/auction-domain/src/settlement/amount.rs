use std::fmt::{Display, Formatter};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use crate::DomainError;

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
