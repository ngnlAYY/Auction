//! v2 处分代：正的安全 JSON 整数。

use serde::Deserialize;
use serde::Serialize;

use crate::dto::public::consignment::wire_v2::SAFE_INTEGER_MAX;
use crate::dto::public::consignment::wire_v2::positive_version;
use crate::dto::public::consignment::wire_v2::serialize_positive_version;

/// 处分代。前三期的解析代从 1 开始，第 4 期的寄售处分代也使用同一安全整数区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingGeneration(i64);

impl ListingGeneration {
    /// 从已校验的安全整数构造；读投影只在正数范围内解封。
    pub const fn new(value: i64) -> Option<Self> {
        if value >= 1 && value <= SAFE_INTEGER_MAX { Some(Self(value)) } else { None }
    }

    /// 返回处分代整数。
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ListingGeneration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        positive_version(deserializer).map(Self)
    }
}

impl Serialize for ListingGeneration {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_positive_version(&self.0, serializer)
    }
}
