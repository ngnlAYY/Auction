//! 排期来源本地钟点的 JSON 表示：`YYYY-MM-DDTHH:MM:SS`，非整秒时追加 3、6 或 9 位小数。
//!
//! 冻结来源随轮次快照与命令账本持久化；写出的文本逐字节保持既有格式，
//! 已入库的快照与按其计算的摘要不因时间库更换而改变。

use std::fmt;

use jiff::civil;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;
use serde::de::Error;

pub(super) fn serialize<S: Serializer>(
    value: &civil::DateTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&LocalText(*value))
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<civil::DateTime, D::Error> {
    let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
    text.parse().map_err(D::Error::custom)
}

struct LocalText(civil::DateTime);

impl fmt::Display for LocalText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0;
        // 公元 0–9999 年之外的年份带符号、至少四位，与既有文本一致。
        if (0..=9999).contains(&value.year()) {
            write!(formatter, "{:04}", value.year())?;
        } else {
            write!(formatter, "{:+05}", value.year())?;
        }
        write!(
            formatter,
            "-{:02}-{:02}T{:02}:{:02}:{:02}",
            value.month(),
            value.day(),
            value.hour(),
            value.minute(),
            value.second()
        )?;
        let nanos = value.subsec_nanosecond();
        if nanos == 0 {
            Ok(())
        } else if nanos % 1_000_000 == 0 {
            write!(formatter, ".{:03}", nanos / 1_000_000)
        } else if nanos % 1_000 == 0 {
            write!(formatter, ".{:06}", nanos / 1_000)
        } else {
            write!(formatter, ".{nanos:09}")
        }
    }
}
