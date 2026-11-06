//! Sent 列表投影的纯逻辑边界：收件人截断前缀、离线大小写与分钟精度时间围栏。
//!
//! 列表只会显示截断后的收件人，完整身份必须来自详情；这两个 helper 只回答
//! “列表是否可能指同一收件人/同一发送分钟”，不构成发送或领取证明。

use auction_application::mail::{mail_recipient_is_list_projection, sent_list_minute_start};
use chrono::{DateTime, Utc};

fn at(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap_or_else(|error| panic!("测试时间必须合法: {error:?}"))
        .with_timezone(&Utc)
}

#[test]
fn list_recipient_projection_accepts_only_a_prefix_of_the_full_identity() {
    // 真实列表会截断到 15 个字符，例如 Restia﹒Ashdoll 显示为 Restia﹒Ashdol。
    let truncated = "Restia\u{fe52}Ashdol";
    let full = "Restia\u{fe52}Ashdoll";
    assert!(mail_recipient_is_list_projection(truncated, full));
    assert!(mail_recipient_is_list_projection(full, full));
    // 仅忽略 ASCII 大小写，不折叠非 ASCII 字符。
    assert!(mail_recipient_is_list_projection("restia\u{fe52}ashdol", full));
    assert!(!mail_recipient_is_list_projection("Restia\u{fe52}Ashdoly", full));
    assert!(!mail_recipient_is_list_projection("OtherTrader", full));
    assert!(!mail_recipient_is_list_projection("", full));
    assert!(!mail_recipient_is_list_projection(truncated, ""));
}

#[test]
fn list_recipient_projection_strips_page_ellipsis_and_replacement_marks() {
    let full = "Clo16TestUserLongName";
    assert!(mail_recipient_is_list_projection("Clo16TestUser\u{2026}", full));
    assert!(mail_recipient_is_list_projection("Clo16TestUser...", full));
    assert!(mail_recipient_is_list_projection("Clo16TestUser\u{fffd}", full));
    // 只有省略号没有实际前缀时不能视作任何收件人。
    assert!(!mail_recipient_is_list_projection("\u{2026}", full));
    assert!(!mail_recipient_is_list_projection("\u{fffd}", full));
}

#[test]
fn sent_list_minute_truncates_to_the_start_of_the_displayed_minute() {
    assert_eq!(sent_list_minute_start(at("2026-11-02T01:14:23.987Z")), at("2026-11-02T01:14:00Z"));
    assert_eq!(sent_list_minute_start(at("2026-11-02T01:14:00Z")), at("2026-11-02T01:14:00Z"));
    // 列表时间早于发送开始分钟时，收缩后仍必须早于开始围栏，不能放宽自动恢复。
    assert!(sent_list_minute_start(at("2026-11-02T01:14:59.999Z")) < at("2026-11-02T01:15:00Z"));
}
