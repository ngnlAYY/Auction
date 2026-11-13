//! 日历触发、并集日规则与夏令时缺口/回拨的固定期望回归。

use auction_application::scheduling::RecurringSchedule;
use chrono::{DateTime, SecondsFormat, Utc};

use super::{StandardCronSchedule, StandardCronScheduleError};

fn next(expression: &str, now: &str) -> Option<String> {
    let schedule = StandardCronSchedule::parse(expression).unwrap();
    let now = DateTime::parse_from_rfc3339(now).unwrap().with_timezone(&Utc);
    schedule.next_after(now).map(|next| next.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn assert_next(expression: &str, now: &str, expected: &str) {
    assert_eq!(next(expression, now).as_deref(), Some(expected), "{expression} after {now}");
}

#[test]
fn interval_schedule_returns_the_first_minute_strictly_after_now() {
    assert_next("TZ=UTC */5 * * * *", "2026-11-13T12:02:30Z", "2026-11-13T12:05:00Z");
    assert_next("TZ=UTC */5 * * * *", "2026-11-13T12:04:59.5Z", "2026-11-13T12:05:00Z");
    // 恰好落在触发时刻时不重复返回当前时刻。
    assert_next("TZ=UTC */5 * * * *", "2026-11-13T12:05:00Z", "2026-11-13T12:10:00Z");
    assert_next("TZ=UTC 59 23 * * *", "2026-12-31T23:59:00.1Z", "2027-01-01T23:59:00Z");
}

#[test]
fn restricted_day_fields_form_a_union_and_wildcards_defer_to_the_other_field() {
    // 2026-11-13 与 2026-12-11 都是星期五。
    assert_next("TZ=UTC 0 0 13 * 5", "2026-11-13T00:00:00Z", "2026-11-20T00:00:00Z");
    assert_next("TZ=UTC 0 0 13 * 5", "2026-12-11T00:00:00Z", "2026-12-13T00:00:00Z");
    assert_next("TZ=UTC 0 0 * * 5", "2026-12-11T00:00:00Z", "2026-12-18T00:00:00Z");
    assert_next("TZ=UTC 0 0 ? * fri", "2026-12-11T00:00:00Z", "2026-12-18T00:00:00Z");
    assert_next("TZ=UTC 0 0 13 * *", "2026-12-11T00:00:00Z", "2026-12-13T00:00:00Z");
    // 显式写满的日字段不是通配，与星期取并集后每天都触发。
    assert_next("TZ=UTC 0 0 1-31 * 5", "2026-12-11T00:00:00Z", "2026-12-12T00:00:00Z");
}

#[test]
fn names_descriptors_and_sparse_dates_resolve_on_the_calendar() {
    assert_next("TZ=UTC 30 9 * JAN mon", "2026-11-13T00:00:00Z", "2027-01-04T09:30:00Z");
    assert_next("TZ=UTC @daily", "2026-11-13T12:00:00Z", "2026-11-14T00:00:00Z");
    assert_next("TZ=UTC @weekly", "2026-11-13T12:00:00Z", "2026-11-15T00:00:00Z");
    assert_next("TZ=UTC @yearly", "2026-11-13T12:00:00Z", "2027-01-01T00:00:00Z");
    assert_next("TZ=UTC 0 0 29 2 *", "2026-11-13T00:00:00Z", "2028-02-29T00:00:00Z");
    // 2100 年不是闰年，相邻两个 2 月 29 日相隔 8 年，仍须在搜索范围内。
    assert_next("TZ=UTC 0 0 29 2 *", "2096-02-29T00:00:00Z", "2104-02-29T00:00:00Z");
    // 语法合法但日历上不存在的日期永不触发；配置校验据此拒绝。
    assert_eq!(next("TZ=UTC 0 0 31 2 *", "2026-11-13T00:00:00Z"), None);
}

#[test]
fn fixed_time_in_a_dst_gap_runs_at_the_first_real_instant_after_it() {
    // Europe/Paris 2025-03-30 02:00 直接跳到 03:00（01:00Z）。
    let schedule = "CRON_TZ=Europe/Paris 30 2 * * *";
    assert_next(schedule, "2025-03-30T00:59:59Z", "2025-03-30T01:00:00Z");
    assert_next(schedule, "2025-03-30T01:00:00Z", "2025-03-31T00:30:00Z");
    assert_next(schedule, "2025-03-28T12:00:00Z", "2025-03-29T01:30:00Z");
}

#[test]
fn interval_schedule_skips_wall_clock_times_inside_a_dst_gap() {
    assert_next(
        "CRON_TZ=Europe/Paris 0,30 2 * * *",
        "2025-03-30T00:59:59Z",
        "2025-03-31T00:00:00Z",
    );
    assert_next(
        "CRON_TZ=Europe/Paris */30 * * * *",
        "2025-03-30T00:30:00Z",
        "2025-03-30T01:00:00Z",
    );
}

#[test]
fn fixed_time_in_a_dst_overlap_runs_once_at_the_earlier_instant() {
    // Europe/Paris 2025-10-26 03:00 回拨到 02:00（01:00Z），02:30 出现两次。
    let schedule = "CRON_TZ=Europe/Paris 30 2 * * *";
    assert_next(schedule, "2025-10-25T23:00:00Z", "2025-10-26T00:30:00Z");
    assert_next(schedule, "2025-10-26T00:30:00Z", "2025-10-27T01:30:00Z");
    assert_next(schedule, "2025-10-26T01:10:00Z", "2025-10-27T01:30:00Z");
}

#[test]
fn interval_schedule_runs_on_both_passes_of_a_dst_overlap() {
    let schedule = "CRON_TZ=Europe/Paris */30 * * * *";
    assert_next(schedule, "2025-10-26T00:10:00Z", "2025-10-26T00:30:00Z");
    assert_next(schedule, "2025-10-26T00:30:00Z", "2025-10-26T01:00:00Z");
    assert_next(schedule, "2025-10-26T01:00:00Z", "2025-10-26T01:30:00Z");
    assert_next(schedule, "2025-10-26T01:30:00Z", "2025-10-26T02:00:00Z");
    // 当前处于前一次的 02:30；后一次会重现 02:15，它早于回拨结束后的 03:15。
    assert_next("CRON_TZ=Europe/Paris 15 * * * *", "2025-10-26T00:30:00Z", "2025-10-26T01:15:00Z");
    assert_next("CRON_TZ=Europe/Paris 15 * * * *", "2025-10-26T01:15:00Z", "2025-10-26T02:15:00Z");
}

#[test]
fn fixed_time_on_a_skipped_calendar_day_moves_to_the_next_scheduled_day() {
    // Pacific/Apia 在 2011 年整日跳过 12 月 30 日；缺口超过两小时的那天不触发，日程继续。
    assert_next("TZ=Pacific/Apia 17 0 * * *", "2011-12-30T02:38:36Z", "2011-12-30T10:17:00Z");
}

#[test]
fn every_descriptor_keeps_whole_second_utc_delays() {
    assert_next("@every 90s", "2026-11-13T12:00:00.4Z", "2026-11-13T12:01:30Z");
    assert_next("@every 1h30m", "2026-11-13T12:00:00Z", "2026-11-13T13:30:00Z");
}

#[test]
fn malformed_expressions_and_timezones_fail_closed() {
    for expression in
        ["* * * *", "* * * * * *", "60 * * * *", "* 24 * * *", "* * 0 * *", "@unknown"]
    {
        assert_eq!(
            StandardCronSchedule::parse(expression).err(),
            Some(StandardCronScheduleError::InvalidExpression),
            "{expression}"
        );
    }
    assert_eq!(
        StandardCronSchedule::parse("TZ=Nowhere/Missing * * * * *").err(),
        Some(StandardCronScheduleError::InvalidTimezone)
    );
}
