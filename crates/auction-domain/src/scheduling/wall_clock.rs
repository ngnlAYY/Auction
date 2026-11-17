//! IANA 时区名与排期钟点的解析；排期各模块与配置层共用同一口径。

use auction_time::parse_clock;
use jiff::tz::TimeZone;

/// 按 IANA 名称查找时区；名称须与内嵌时区数据库里的写法逐字符一致。
///
/// 数据库查找本身不区分大小写，这里补上精确比较：配置与冻结来源里的时区名就是规范写法，
/// 大小写不同的名字照旧拒绝。`Factory` 是数据库里表示“尚未设置时区”的占位条目，不是可用时区。
pub fn parse_timezone(name: &str) -> Option<TimeZone> {
    if name == "Factory" {
        return None;
    }
    let timezone = TimeZone::get(name).ok()?;
    (timezone.iana_name() == Some(name)).then_some(timezone)
}

/// 解析 `HH:MM` 并返回它是当天的第几分钟（0 到 1439）。
///
/// 部署配置一直按宽松范围书写钟点（例如 `9:05`），这里不另加补零限制。
pub fn parse_clock_minute(value: &str) -> Option<u16> {
    let time = parse_clock(value)?;
    u16::try_from(i32::from(time.hour()) * 60 + i32::from(time.minute())).ok()
}
