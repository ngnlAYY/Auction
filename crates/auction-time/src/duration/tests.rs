//! 时长的文本输出、截断方向与除法舍入的回归；期望值取自迁移前的历史实现。

use super::Duration;

/// (秒, 纳秒, Display, Debug, 整秒, 整毫秒, 除以 7 的纳秒数)
const GOLDEN: [(i64, i64, &str, &str, i64, i64, i64); 6] = [
    (0, 0, "P0D", "TimeDelta { secs: 0, nanos: 0 }", 0, 0, 0),
    (
        90_061,
        500_000_000,
        "PT90061.5S",
        "TimeDelta { secs: 90061, nanos: 500000000 }",
        90_061,
        90_061_500,
        12_865_928_571_428,
    ),
    (-2, 0, "-PT2S", "TimeDelta { secs: -2, nanos: 0 }", -2, -2_000, -285_714_285),
    (0, -500_000_000, "-PT0.5S", "TimeDelta { secs: -1, nanos: 500000000 }", 0, -500, -71_428_571),
    (
        -6_744_890,
        513_707_674,
        "-PT6744889.486292326S",
        "TimeDelta { secs: -6744890, nanos: 513707674 }",
        -6_744_889,
        -6_744_889_486,
        -963_555_640_898_904,
    ),
    (5, 1, "PT5.000000001S", "TimeDelta { secs: 5, nanos: 1 }", 5, 5_000, 714_285_714),
];

#[test]
fn text_truncation_and_division_are_stable() {
    for (seconds, nanoseconds, display, debug, whole_seconds, whole_millis, sevenths) in GOLDEN {
        let duration = Duration::seconds(seconds) + Duration::nanoseconds(nanoseconds);
        assert_eq!(duration.to_string(), display);
        assert_eq!(format!("{duration:?}"), debug);
        assert_eq!(duration.num_seconds(), whole_seconds);
        assert_eq!(duration.num_milliseconds(), whole_millis);
        assert_eq!((duration / 7).num_nanoseconds(), Some(sevenths));
    }
}

#[test]
fn units_and_standard_library_conversion() {
    assert_eq!(Duration::minutes(2), Duration::seconds(120));
    assert_eq!(Duration::hours(1), Duration::minutes(60));
    assert_eq!(Duration::days(1), Duration::hours(24));
    assert_eq!(Duration::weeks(1), Duration::days(7));
    assert_eq!(Duration::days(3).num_days(), 3);
    assert_eq!(Duration::seconds(-90).num_minutes(), -1);
    assert_eq!(Duration::milliseconds(-1_500).abs(), Duration::milliseconds(1_500));
    assert_eq!(Duration::seconds(3) * 4, Duration::seconds(12));
    assert_eq!(-Duration::seconds(3), Duration::seconds(-3));
    assert!(Duration::zero().is_zero());
    assert_eq!(Duration::default(), Duration::ZERO);
    assert_eq!(
        [Duration::seconds(1), Duration::seconds(2)].into_iter().sum::<Duration>(),
        Duration::seconds(3)
    );

    assert_eq!(
        Duration::milliseconds(1_500).to_std().unwrap(),
        std::time::Duration::from_millis(1_500)
    );
    assert!(Duration::seconds(-1).to_std().is_err());
    assert_eq!(
        Duration::from_std(std::time::Duration::from_secs(5)).unwrap(),
        Duration::seconds(5)
    );
    assert!(Duration::from_std(std::time::Duration::MAX).is_err());
}

#[test]
fn serde_form_is_a_floor_seconds_and_nanoseconds_pair() {
    let cases = [
        (Duration::seconds(90_061) + Duration::milliseconds(500), "[90061,500000000]"),
        (Duration::milliseconds(-500), "[-1,500000000]"),
        (Duration::zero(), "[0,0]"),
        (Duration::seconds(-2), "[-2,0]"),
    ];
    for (duration, json) in cases {
        assert_eq!(serde_json::to_string(&duration).unwrap(), json);
        assert_eq!(serde_json::from_str::<Duration>(json).unwrap(), duration);
    }
    // 纳秒须在一秒之内，毫秒数须能用 i64 表示。
    assert!(serde_json::from_str::<Duration>("[0,1000000000]").is_err());
    assert!(serde_json::from_str::<Duration>("[0,-1]").is_err());
    assert!(serde_json::from_str::<Duration>("[9223372036854776,0]").is_err());
    assert!(serde_json::from_str::<Duration>("[9223372036854775,807000000]").is_ok());
    assert!(serde_json::from_str::<Duration>("[9223372036854775,807000001]").is_err());
    assert!(Duration::try_days(i64::MAX).is_none());
    assert_eq!(Duration::try_days(2), Some(Duration::days(2)));
}
