//! 出站通知正文共用的确定性文案片段：lot 代码、千分位金额与截止时刻。
//!
//! 只做纯格式化，不读取配置、时钟或数据库；同一输入必须产生同一冻结正文，
//! 否则重放时请求摘要会与已入队通知冲突。

use auction_domain::equipment::taxonomy::EquipmentCategory;
use auction_time::{SecondsFormat, Timestamp};

/// 未知装备分类在展示代码中使用的可读前缀。
const UNKNOWN_LOT_CODE_PREFIX: &str = "Lot";

/// 三字母前缀加至少两位补零的类别内编号。已知大类的代码由领域唯一定义，邮件、网站、竞价日志
/// 与论坛目录显示同一代码；这里只补未知分类的可读回退。论坛可竞价代码另由论坛同步失败关闭地生成。
pub fn lot_code(category: &str, lot_number: impl Into<i64>) -> String {
    let lot_number = lot_number.into();
    EquipmentCategory::from_name(category).map_or_else(
        || format!("{UNKNOWN_LOT_CODE_PREFIX}{lot_number:02}"),
        |category| category.lot_code(lot_number),
    )
}

/// 千分位完整金额；出站邮件不使用 K/M 近似值。负数先按绝对值分组再补符号，
/// 避免负号被当成一位数字参与分组。
pub fn format_credits(amount: i64) -> String {
    let digits = amount.unsigned_abs().to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if amount < 0 {
        formatted.push('-');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

/// 以 UTC 秒精度书写的有效截止时刻；正文不再换算玩家本地时区。
pub fn format_deadline(deadline: Timestamp) -> String {
    deadline.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;

    use super::{format_credits, format_deadline, lot_code};

    #[test]
    fn lot_code_matches_forum_prefixes_and_default() {
        assert_eq!(lot_code("Cloth", 16), "Clo16");
        assert_eq!(lot_code("One-Handed", 1), "One01");
        assert_eq!(lot_code("Heavy", 120), "Hea120");
        assert_eq!(lot_code("Unknown", 3), "Lot03");
    }

    #[test]
    fn credits_use_thousands_separators() {
        assert_eq!(format_credits(0), "0");
        assert_eq!(format_credits(999), "999");
        assert_eq!(format_credits(1_000), "1,000");
        assert_eq!(format_credits(5_252_000), "5,252,000");
        assert_eq!(format_credits(1_000_000_000), "1,000,000,000");
    }

    #[test]
    fn negative_credits_group_the_absolute_value() {
        assert_eq!(format_credits(-100), "-100");
        assert_eq!(format_credits(-1_000), "-1,000");
        assert_eq!(format_credits(-123_456), "-123,456");
        assert_eq!(format_credits(i64::MIN), "-9,223,372,036,854,775,808");
    }

    #[test]
    fn deadline_is_frozen_utc_seconds() {
        let deadline = Timestamp::from_ymd_hms(2026, 11, 5, 12, 30, 5).unwrap();
        assert_eq!(format_deadline(deadline), "2026-11-05T12:30:05Z");
    }
}
