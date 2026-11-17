//! Sent 列表投影的纯逻辑边界：收件人截断前缀、离线大小写与分钟精度时间围栏。
//!
//! 列表只会显示截断后的收件人，完整身份必须来自详情；这两个 helper 只回答
//! “列表是否可能指同一收件人/同一发送分钟”，不构成发送或领取证明。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_application::mail::{mail_recipient_is_list_projection, sent_list_minute_start};
use auction_time::Timestamp;

fn at(value: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(value)
        .unwrap_or_else(|error| panic!("测试时间必须合法: {error:?}"))
}

#[test]
fn list_recipient_projection_accepts_only_a_prefix_of_the_full_identity() {
    // 列表按 15 字节截断收件人：完整名 Sample﹒Fixture（14 字符/16 字节，第 7 位是 3 字节字符）
    // 显示为 Sample﹒Fixtur（13 字符/15 字节）。
    let truncated = "Sample\u{fe52}Fixtur";
    let full = "Sample\u{fe52}Fixture";
    assert!(mail_recipient_is_list_projection(truncated, full));
    assert!(mail_recipient_is_list_projection(full, full));
    // 仅忽略 ASCII 大小写，不折叠非 ASCII 字符。
    assert!(mail_recipient_is_list_projection("sample\u{fe52}fixtur", full));
    assert!(!mail_recipient_is_list_projection("Sample\u{fe52}Fixtury", full));
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
