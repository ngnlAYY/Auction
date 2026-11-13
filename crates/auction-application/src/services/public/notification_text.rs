//! 出站通知正文共用的确定性文案片段：lot 代码、千分位金额与截止时刻。
//!
//! 只做纯格式化，不读取配置、时钟或数据库；同一输入必须产生同一冻结正文，
//! 否则重放时请求摘要会与已入队通知冲突。

use chrono::{DateTime, SecondsFormat, Utc};

/// 装备大类到论坛 lot 代码前缀的映射；未知分类保留可读的 `Lot` 前缀。
///
/// 该映射与论坛目录和公开竞价日志使用同一历史前缀，因此玩家在邮件里看到的
/// 代码与网站、论坛完全一致。
pub fn lot_code_prefix(category: &str) -> &'static str {
    match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => "Lot",
    }
}

/// 三字母前缀加两位编号；编号来自 lot 的稳定 `lot_number`。
pub fn lot_code(category: &str, lot_number: u32) -> String {
    format!("{}{lot_number:02}", lot_code_prefix(category))
}

/// 千分位完整金额；出站邮件不使用 K/M 近似值。
pub fn format_credits(amount: i64) -> String {
    let digits = amount.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

/// 以 UTC 秒精度书写的有效截止时刻；正文不再换算玩家本地时区。
pub fn format_deadline(deadline: DateTime<Utc>) -> String {
    deadline.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::services::public::notification_text as parent;
    use chrono::TimeZone;

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
    fn deadline_is_frozen_utc_seconds() {
        let deadline = Utc.with_ymd_and_hms(2026, 11, 5, 12, 30, 5).unwrap();
        assert_eq!(format_deadline(deadline), "2026-11-05T12:30:05Z");
    }
}
