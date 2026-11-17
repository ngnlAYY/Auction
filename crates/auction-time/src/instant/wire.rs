//! 时刻的 serde 表示：RFC 3339 字符串。
//!
//! 输出固定为 `Z` 结尾、小数位取 0/3/6/9 位，与已入库 JSON 及其摘要一致；输入接受宽松的
//! RFC 3339（见 [`Timestamp`] 的 `FromStr`），带偏移的时间换算成 UTC。

use std::fmt;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de;

use super::Timestamp;
use super::format::Rfc3339;
use super::format::SecondsFormat;

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&Rfc3339 {
            timestamp: *self,
            seconds_format: SecondsFormat::AutoSi,
            use_z: true,
        })
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(TimestampVisitor)
    }
}

struct TimestampVisitor;

impl de::Visitor<'_> for TimestampVisitor {
    type Value = Timestamp;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an RFC 3339 formatted date and time string")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Timestamp, E> {
        value.parse().map_err(E::custom)
    }
}
