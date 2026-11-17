//! 时长的 serde 表示：`[秒, 纳秒]` 二元组，秒向下取整、纳秒非负。
//!
//! 已冻结的场次策略以这种形式入库，输出逐字节固定；读取时沿用历史的取值范围
//! （毫秒数须能用 `i64` 表示），范围之外按错误拒绝。

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::Error;

use super::Duration;

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.floor_parts().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (seconds, nanoseconds) = <(i64, i32)>::deserialize(deserializer)?;
        Self::from_floor_parts(seconds, nanoseconds)
            .ok_or_else(|| D::Error::custom("TimeDelta out of bounds"))
    }
}
