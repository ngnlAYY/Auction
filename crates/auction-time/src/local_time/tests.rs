//! 墙上时间解析的回归：前跳、回拨，以及时区数据误报的重复时刻。

use jiff::civil;
use jiff::tz::Offset;
use jiff::tz::TimeZone;

use super::LocalTime;
use crate::instant::Timestamp;

fn zone(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap()
}

fn utc(text: &str) -> Timestamp {
    Timestamp::parse_from_rfc3339(text).unwrap()
}

#[test]
fn ordinary_wall_clock_time_maps_to_one_instant() {
    let resolved =
        LocalTime::resolve(&zone("America/New_York"), civil::datetime(2026, 7, 1, 12, 0, 0, 0));
    assert_eq!(resolved, LocalTime::Unique(utc("2026-07-01T16:00:00Z")));
}

#[test]
fn spring_forward_gap_reports_the_offset_after_the_jump() {
    let resolved =
        LocalTime::resolve(&zone("America/New_York"), civil::datetime(2026, 3, 8, 2, 30, 0, 0));
    let after = Offset::from_seconds(-4 * 3600).unwrap();
    assert_eq!(resolved, LocalTime::Skipped { after });
}

#[test]
fn fall_back_fold_reports_both_instants_in_order() {
    let resolved =
        LocalTime::resolve(&zone("America/New_York"), civil::datetime(2026, 11, 1, 1, 30, 0, 0));
    assert_eq!(
        resolved,
        LocalTime::Repeated {
            earlier: utc("2026-11-01T05:30:00Z"),
            later: utc("2026-11-01T06:30:00Z"),
        }
    );
}

/// 格陵兰 2023 年 10 月只切换规则、偏移保持 -02:00；时区库按本地时间查找时会多报一个
/// -01:00 的重复时刻，那个时刻从未存在。
#[test]
fn rule_change_without_offset_change_is_not_a_fold() {
    for name in ["America/Nuuk", "America/Godthab"] {
        let resolved = LocalTime::resolve(&zone(name), civil::datetime(2023, 10, 28, 23, 30, 0, 0));
        assert_eq!(resolved, LocalTime::Unique(utc("2023-10-29T01:30:00Z")), "{name}");
    }
}
