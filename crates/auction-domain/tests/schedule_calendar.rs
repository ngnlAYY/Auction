//! 排期日历的纯逻辑回归：IANA 时区 DST 解析、跨 DST 后继、早鸟窗口与赛季前场次、冻结来源校验。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use auction_domain::{
    AuctionEarlyBirdConfig, AuctionEarlyBirdSlot, AuctionScheduleSource, AuctionScheduleTimes,
    DomainError,
};
use auction_time::Timestamp;
use jiff::civil;

fn required<T>(value: Option<T>) -> T {
    value.unwrap_or_else(|| panic!("测试夹具必须存在"))
}

fn naive(year: i16, month: i8, day: i8, hour: i8, minute: i8) -> civil::DateTime {
    civil::DateTime::new(year, month, day, hour, minute, 0, 0)
        .unwrap_or_else(|error| panic!("测试夹具必须存在: {error}"))
}

fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> Timestamp {
    required(Timestamp::from_ymd_hms(year, month, day, hour, minute, 0))
}

fn source(
    timezone: &str,
    preview: civil::DateTime,
    open: civil::DateTime,
    end: civil::DateTime,
) -> AuctionScheduleSource {
    AuctionScheduleSource {
        timezone: timezone.to_owned(),
        preview_local: preview,
        open_local: open,
        end_local: end,
        early_bird: None,
    }
}

#[test]
fn weekly_source_resolves_spring_forward_and_fall_back_coerced_wall_clocks() {
    // 2026-03-08 02:30 在 America/New_York 不存在：按跳时后的 offset 归一化到 06:30Z。
    let spring = source(
        "America/New_York",
        naive(2026, 3, 8, 1, 0),
        naive(2026, 3, 8, 2, 30),
        naive(2026, 3, 8, 5, 0),
    );
    let times = spring.resolve().expect("DST 跳时仍必须形成可物化时段");
    assert_eq!(times.preview_due_at, utc(2026, 3, 8, 6, 0));
    assert_eq!(times.open_due_at, utc(2026, 3, 8, 6, 30));
    assert_eq!(times.scheduled_end_at, utc(2026, 3, 8, 9, 0));

    // 2026-11-01 01:30 出现两次：歧义墙上时间取较早实例（EDT）。
    let fall = source(
        "America/New_York",
        naive(2026, 11, 1, 0, 30),
        naive(2026, 11, 1, 1, 30),
        naive(2026, 11, 1, 3, 0),
    );
    let times = fall.resolve().expect("回拨歧义时间必须按较早实例解析");
    assert_eq!(times.preview_due_at, utc(2026, 11, 1, 4, 30));
    assert_eq!(times.open_due_at, utc(2026, 11, 1, 5, 30));
    assert_eq!(times.scheduled_end_at, utc(2026, 11, 1, 8, 0));
}

#[test]
fn next_week_keeps_the_wall_clock_across_dst() {
    let before = source(
        "America/New_York",
        naive(2026, 3, 1, 16, 0),
        naive(2026, 3, 1, 18, 0),
        naive(2026, 3, 1, 20, 0),
    );
    let before_times = before.resolve().unwrap();
    assert_eq!(before_times.open_due_at, utc(2026, 3, 1, 23, 0));

    let after = before.next_week().expect("周后继必须按本地日历推进");
    assert_eq!(after.open_local, naive(2026, 3, 8, 18, 0));
    let after_times = after.resolve().unwrap();
    // 同一墙上钟点跨过前跳，UTC 间隔因此是 167 小时而不是 168。
    assert_eq!(after_times.open_due_at, utc(2026, 3, 8, 22, 0));
    assert_eq!(
        after_times.open_due_at - before_times.open_due_at,
        auction_time::Duration::hours(167)
    );
}

#[test]
fn frozen_source_rejects_invalid_timezone_and_non_increasing_times() {
    let invalid_zone = source(
        "Not/AZone",
        naive(2026, 1, 1, 1, 0),
        naive(2026, 1, 1, 2, 0),
        naive(2026, 1, 1, 3, 0),
    );
    assert!(matches!(invalid_zone.resolve(), Err(DomainError::InvalidSchedule(_))));

    let unordered =
        source("UTC", naive(2026, 1, 1, 2, 0), naive(2026, 1, 1, 2, 0), naive(2026, 1, 1, 3, 0));
    assert!(matches!(unordered.resolve(), Err(DomainError::InvalidSchedule(_))));
}

#[test]
fn early_bird_sessions_cover_window_pre_season_and_intake_boundaries() {
    let config = AuctionEarlyBirdConfig { enabled: true, ..AuctionEarlyBirdConfig::default() };
    let calendar = config.active_calendar().unwrap().expect("启用的早鸟配置必须生成日历");
    let season = civil::date(2026, 11, 1);

    let first = calendar.session(season, 1).expect("窗口首场必须可物化");
    assert_eq!(first.times.preview_due_at, utc(2026, 11, 1, 16, 0));
    assert_eq!(first.times.open_due_at, utc(2026, 11, 1, 18, 0));
    assert_eq!(first.times.scheduled_end_at, utc(2026, 11, 2, 18, 0));
    assert_eq!(first.source.early_bird, Some(AuctionEarlyBirdSlot { season, session: 1 }));
    // 窗口内场次在赛季开始前不接收装备。
    assert_eq!(
        calendar.intake_opens_at(first.source.early_bird.unwrap()).unwrap(),
        Some(utc(2026, 11, 1, 9, 0))
    );

    let pre = calendar.session(season, 0).expect("默认配置必须物化赛季前场次");
    assert_eq!(pre.times.open_due_at, utc(2026, 10, 29, 18, 0));
    assert_eq!(pre.times.scheduled_end_at, utc(2026, 10, 30, 18, 0));
    assert!(pre.source.early_bird.unwrap().is_pre_season());
    assert_eq!(calendar.intake_opens_at(pre.source.early_bird.unwrap()).unwrap(), None);
    assert!(matches!(calendar.session(season, 99), Err(DomainError::InvalidSchedule(_))));

    assert!(calendar.window_contains("isekai", utc(2026, 11, 2, 0, 0)));
    assert!(!calendar.window_contains("main", utc(2026, 11, 2, 0, 0)));
    assert!(!calendar.window_contains("isekai", utc(2026, 11, 8, 9, 0)));
    assert!(calendar.window_contains("isekai", utc(2026, 11, 8, 8, 59)));
}

#[test]
fn early_bird_replaces_only_overlapping_weekly_slots_in_participating_worlds() {
    let config = AuctionEarlyBirdConfig { enabled: true, ..AuctionEarlyBirdConfig::default() };
    let calendar = config.active_calendar().unwrap().unwrap();

    let overlapping = AuctionScheduleTimes {
        preview_due_at: utc(2026, 10, 31, 17, 0),
        open_due_at: utc(2026, 10, 31, 19, 0),
        scheduled_end_at: utc(2026, 11, 1, 19, 0),
    };
    assert!(calendar.suppresses("isekai", &overlapping));
    assert!(!calendar.suppresses("main", &overlapping));

    let far_away = AuctionScheduleTimes {
        preview_due_at: utc(2026, 9, 1, 17, 0),
        open_due_at: utc(2026, 9, 1, 19, 0),
        scheduled_end_at: utc(2026, 9, 2, 19, 0),
    };
    assert!(!calendar.suppresses("isekai", &far_away));
}

#[test]
fn early_bird_window_uses_local_dst_boundaries() {
    let config = AuctionEarlyBirdConfig {
        enabled: true,
        timezone: "America/New_York".to_owned(),
        ..AuctionEarlyBirdConfig::default()
    };
    let calendar = config.active_calendar().unwrap().unwrap();
    let season = civil::date(2026, 11, 1);

    // 2026-11-01 当地时间 02:00 回拨；09:00 已是 EST，窗口从 14:00Z 开始。
    let first = calendar.session(season, 1).unwrap();
    assert_eq!(first.times.open_due_at, utc(2026, 11, 1, 23, 0));
    assert!(calendar.window_contains("isekai", utc(2026, 11, 1, 14, 0)));
    assert!(!calendar.window_contains("isekai", utc(2026, 11, 1, 13, 59)));
}

#[test]
fn early_bird_config_rejects_cross_field_violations_even_when_disabled() {
    let short_preview =
        AuctionEarlyBirdConfig { preview_lead_minutes: 60, ..AuctionEarlyBirdConfig::default() };
    assert_eq!(short_preview.validate(), Err("preview_lead_minutes"));
    // enabled=false 也必须先完整校验，保存的候选配置不能只在启用时才失败。
    assert!(short_preview.active_calendar().is_err());

    let duplicate_days = AuctionEarlyBirdConfig {
        session_days: vec![1, 3, 3, 7],
        ..AuctionEarlyBirdConfig::default()
    };
    assert_eq!(duplicate_days.validate(), Err("session_days"));

    let no_world =
        AuctionEarlyBirdConfig { worlds: Vec::new(), ..AuctionEarlyBirdConfig::default() };
    assert_eq!(no_world.validate(), Err("worlds"));

    let leap_day = AuctionEarlyBirdConfig {
        season_starts: vec!["02-29".to_owned()],
        ..AuctionEarlyBirdConfig::default()
    };
    assert_eq!(leap_day.validate(), Err("season_starts"));

    // 赛季前场次结束加 24 小时付款期限必须早于换季日本地 00:00。
    let late_pre_season =
        AuctionEarlyBirdConfig { pre_season_days_before: 1, ..AuctionEarlyBirdConfig::default() };
    assert_eq!(late_pre_season.validate(), Err("pre_season_days_before"));
}

#[test]
fn early_bird_slot_is_not_a_weekly_series() {
    let config = AuctionEarlyBirdConfig { enabled: true, ..AuctionEarlyBirdConfig::default() };
    let slot =
        config.active_calendar().unwrap().unwrap().session(civil::date(2026, 11, 1), 1).unwrap();

    assert!(matches!(slot.source.next_week(), Err(DomainError::InvalidSchedule(_))));
}

/// 排期日期限定在公元 1–9999 年：这之外的年份，日期与身份文本的写法与已持久化的格式不一致，
/// 来源直接判为无效，不会被物化、改期或推进。
#[test]
fn schedule_source_rejects_years_outside_the_supported_range() {
    let valid = source("UTC", naive(1, 1, 1, 0, 0), naive(1, 1, 1, 3, 0), naive(1, 1, 2, 3, 0));
    assert!(valid.resolve().is_ok(), "公元 1 年在支持范围内");
    let latest =
        source("UTC", naive(9999, 6, 1, 0, 0), naive(9999, 6, 1, 3, 0), naive(9999, 6, 2, 3, 0));
    assert!(latest.resolve().is_ok(), "公元 9999 年在支持范围内");

    for (preview, open, end) in [
        (naive(0, 12, 31, 20, 0), naive(1, 1, 1, 0, 0), naive(1, 1, 2, 0, 0)),
        (naive(-1, 6, 1, 0, 0), naive(-1, 6, 1, 3, 0), naive(-1, 6, 2, 3, 0)),
    ] {
        let rejected = source("UTC", preview, open, end);
        assert!(matches!(rejected.resolve(), Err(DomainError::InvalidSchedule(_))));
    }
    // 顺延的结果同样受限：从公元 1 年初往回推一周落到公元 0 年，被拒绝。
    assert!(matches!(valid.shifted_local_days(-7), Err(DomainError::InvalidSchedule(_))));
    // 越界的来源也不能靠顺延回到支持范围：公元 0 年末的来源推进一周会落进公元 1 年，仍被拒绝。
    let before_range =
        source("UTC", naive(0, 12, 25, 0, 0), naive(0, 12, 26, 0, 0), naive(0, 12, 27, 0, 0));
    assert!(matches!(before_range.next_week(), Err(DomainError::InvalidSchedule(_))));
    assert!(matches!(before_range.shifted_local_days(7), Err(DomainError::InvalidSchedule(_))));

    let mut early_bird = valid;
    early_bird.early_bird =
        Some(AuctionEarlyBirdSlot { season: civil::date(-1, 11, 1), session: 1 });
    assert!(matches!(early_bird.resolve(), Err(DomainError::InvalidSchedule(_))));
}
