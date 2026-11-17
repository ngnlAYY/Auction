//! 时刻文本契约的回归：输出逐字节固定，解析的接受范围与错误类别固定。
//!
//! 期望值取自迁移前的历史实现；改动这里的任何一个字符串都意味着已入库的摘要会对不上。

use std::str::FromStr;

use super::ParseErrorKind;
use super::SecondsFormat;
use super::Timestamp;
use crate::duration::Duration;

/// (秒, 纳秒, 序列化, to_rfc3339, Display, Secs+Z, Millis+Z, Micros+偏移, 毫秒, 微秒, 秒内纳秒)
type Golden = (
    i64,
    u32,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    i64,
    i64,
    u32,
);

const GOLDEN: [Golden; 9] = [
    (
        0,
        0,
        "1970-01-01T00:00:00Z",
        "1970-01-01T00:00:00+00:00",
        "1970-01-01 00:00:00 UTC",
        "1970-01-01T00:00:00Z",
        "1970-01-01T00:00:00.000Z",
        "1970-01-01T00:00:00.000000+00:00",
        0,
        0,
        0,
    ),
    (
        1_794_909_600,
        0,
        "2026-11-17T10:00:00Z",
        "2026-11-17T10:00:00+00:00",
        "2026-11-17 10:00:00 UTC",
        "2026-11-17T10:00:00Z",
        "2026-11-17T10:00:00.000Z",
        "2026-11-17T10:00:00.000000+00:00",
        1_794_909_600_000,
        1_794_909_600_000_000,
        0,
    ),
    (
        1_794_909_600,
        120_000_000,
        "2026-11-17T10:00:00.120Z",
        "2026-11-17T10:00:00.120+00:00",
        "2026-11-17 10:00:00.120 UTC",
        "2026-11-17T10:00:00Z",
        "2026-11-17T10:00:00.120Z",
        "2026-11-17T10:00:00.120000+00:00",
        1_794_909_600_120,
        1_794_909_600_120_000,
        120_000_000,
    ),
    (
        1_794_909_600,
        123_000_000,
        "2026-11-17T10:00:00.123Z",
        "2026-11-17T10:00:00.123+00:00",
        "2026-11-17 10:00:00.123 UTC",
        "2026-11-17T10:00:00Z",
        "2026-11-17T10:00:00.123Z",
        "2026-11-17T10:00:00.123000+00:00",
        1_794_909_600_123,
        1_794_909_600_123_000,
        123_000_000,
    ),
    (
        1_794_909_600,
        123_450_000,
        "2026-11-17T10:00:00.123450Z",
        "2026-11-17T10:00:00.123450+00:00",
        "2026-11-17 10:00:00.123450 UTC",
        "2026-11-17T10:00:00Z",
        "2026-11-17T10:00:00.123Z",
        "2026-11-17T10:00:00.123450+00:00",
        1_794_909_600_123,
        1_794_909_600_123_450,
        123_450_000,
    ),
    (
        1_794_909_600,
        123_456_789,
        "2026-11-17T10:00:00.123456789Z",
        "2026-11-17T10:00:00.123456789+00:00",
        "2026-11-17 10:00:00.123456789 UTC",
        "2026-11-17T10:00:00Z",
        "2026-11-17T10:00:00.123Z",
        "2026-11-17T10:00:00.123456+00:00",
        1_794_909_600_123,
        1_794_909_600_123_456,
        123_456_789,
    ),
    (
        -1,
        999_999_999,
        "1969-12-31T23:59:59.999999999Z",
        "1969-12-31T23:59:59.999999999+00:00",
        "1969-12-31 23:59:59.999999999 UTC",
        "1969-12-31T23:59:59Z",
        "1969-12-31T23:59:59.999Z",
        "1969-12-31T23:59:59.999999+00:00",
        -1,
        -1,
        999_999_999,
    ),
    (
        -62_135_596_800,
        0,
        "0001-01-01T00:00:00Z",
        "0001-01-01T00:00:00+00:00",
        "0001-01-01 00:00:00 UTC",
        "0001-01-01T00:00:00Z",
        "0001-01-01T00:00:00.000Z",
        "0001-01-01T00:00:00.000000+00:00",
        -62_135_596_800_000,
        -62_135_596_800_000_000,
        0,
    ),
    // 可表示范围的上限附近：底层时刻最大到 9999-12-30T22:00:00.999999999Z。
    (
        253_402_207_199,
        999_999_999,
        "9999-12-30T21:59:59.999999999Z",
        "9999-12-30T21:59:59.999999999+00:00",
        "9999-12-30 21:59:59.999999999 UTC",
        "9999-12-30T21:59:59Z",
        "9999-12-30T21:59:59.999Z",
        "9999-12-30T21:59:59.999999+00:00",
        253_402_207_199_999,
        253_402_207_199_999_999,
        999_999_999,
    ),
];

#[test]
fn text_forms_are_byte_stable() {
    for (
        seconds,
        nanoseconds,
        json,
        rfc3339,
        display,
        secs,
        millis,
        micros,
        as_millis,
        as_micros,
        subsec,
    ) in GOLDEN
    {
        let timestamp = Timestamp::from_timestamp(seconds, nanoseconds).unwrap();
        assert_eq!(serde_json::to_string(&timestamp).unwrap(), format!("\"{json}\""));
        assert_eq!(format!("{timestamp:?}"), json);
        assert_eq!(timestamp.to_rfc3339(), rfc3339);
        assert_eq!(timestamp.to_string(), display);
        assert_eq!(timestamp.to_rfc3339_opts(SecondsFormat::Secs, true), secs);
        assert_eq!(timestamp.to_rfc3339_opts(SecondsFormat::Millis, true), millis);
        assert_eq!(timestamp.to_rfc3339_opts(SecondsFormat::Micros, false), micros);
        assert_eq!(timestamp.timestamp(), seconds);
        assert_eq!(timestamp.timestamp_millis(), as_millis);
        assert_eq!(timestamp.timestamp_micros(), as_micros);
        assert_eq!(timestamp.timestamp_subsec_nanos(), subsec);
        // 序列化文本能原样读回同一时刻。
        assert_eq!(serde_json::from_str::<Timestamp>(&format!("\"{json}\"")).unwrap(), timestamp);
        assert_eq!(Timestamp::parse_from_rfc3339(rfc3339).unwrap(), timestamp);
    }
}

/// (输入, 严格形式的结果, 宽松形式的结果)；成功时给出九位小数的 UTC 文本。
type Parsed =
    (&'static str, Result<&'static str, ParseErrorKind>, Result<&'static str, ParseErrorKind>);

const PARSED: [Parsed; 16] = [
    (
        "2026-11-17T10:00:00Z",
        Ok("2026-11-17T10:00:00.000000000Z"),
        Ok("2026-11-17T10:00:00.000000000Z"),
    ),
    (
        "2026-11-17t10:00:00.5z",
        Ok("2026-11-17T10:00:00.500000000Z"),
        Ok("2026-11-17T10:00:00.500000000Z"),
    ),
    (
        "2026-11-17 18:00:00+08:00",
        Ok("2026-11-17T10:00:00.000000000Z"),
        Ok("2026-11-17T10:00:00.000000000Z"),
    ),
    // 小数秒只取前九位。
    (
        "2026-11-17T10:00:00.123456789123Z",
        Ok("2026-11-17T10:00:00.123456789Z"),
        Ok("2026-11-17T10:00:00.123456789Z"),
    ),
    // 偏移的负号可以是 U+2212。
    (
        "2026-11-17T10:00:00\u{2212}02:30",
        Ok("2026-11-17T12:30:00.000000000Z"),
        Ok("2026-11-17T12:30:00.000000000Z"),
    ),
    // 以下两项只有宽松形式接受：字段不补零与 UTC 字面量、偏移省略冒号并带前导空格。
    ("2026-1-7 3:04:05 UTC", Err(ParseErrorKind::Invalid), Ok("2026-01-07T03:04:05.000000000Z")),
    (
        "2026-11-17T10:00:00 +0800",
        Err(ParseErrorKind::Invalid),
        Ok("2026-11-17T02:00:00.000000000Z"),
    ),
    ("2026-11-17T10:00:00", Err(ParseErrorKind::TooShort), Err(ParseErrorKind::TooShort)),
    ("2026-11-17T10:00:00Zx", Err(ParseErrorKind::TooLong), Err(ParseErrorKind::TooLong)),
    ("2026-02-30T10:00:00Z", Err(ParseErrorKind::OutOfRange), Err(ParseErrorKind::OutOfRange)),
    ("2026-11-17T24:00:00Z", Err(ParseErrorKind::OutOfRange), Err(ParseErrorKind::OutOfRange)),
    // 闰秒是与历史实现唯一的有意差异：历史实现接受并保留 `:60`，这里拒绝。
    ("2026-11-17T10:00:60Z", Err(ParseErrorKind::OutOfRange), Err(ParseErrorKind::OutOfRange)),
    ("2026-11-17T10:00:00+24:00", Err(ParseErrorKind::OutOfRange), Err(ParseErrorKind::OutOfRange)),
    ("2026-11-17", Err(ParseErrorKind::TooShort), Err(ParseErrorKind::TooShort)),
    ("2026-11-17T10:00:00+08", Err(ParseErrorKind::TooShort), Err(ParseErrorKind::TooShort)),
    ("abc", Err(ParseErrorKind::TooShort), Err(ParseErrorKind::Invalid)),
];

#[test]
fn parsing_accepts_and_rejects_the_same_inputs() {
    let render = |result: Result<Timestamp, super::ParseError>| {
        result
            .map(|value| value.to_rfc3339_opts(SecondsFormat::Nanos, true))
            .map_err(|error| error.kind())
    };
    for (text, strict, relaxed) in PARSED {
        assert_eq!(
            render(Timestamp::parse_from_rfc3339(text)),
            strict.map(str::to_owned),
            "严格：{text}"
        );
        assert_eq!(render(Timestamp::from_str(text)), relaxed.map(str::to_owned), "宽松：{text}");
        // 反序列化走宽松形式。
        let json = serde_json::to_string(text).unwrap();
        assert_eq!(
            serde_json::from_str::<Timestamp>(&json).is_ok(),
            relaxed.is_ok(),
            "JSON：{text}"
        );
    }
}

#[test]
fn deserialize_rejects_non_string_values() {
    assert!(serde_json::from_str::<Timestamp>("1794909600").is_err());
    assert!(serde_json::from_str::<Timestamp>("null").is_err());
}

#[test]
fn arithmetic_and_range_checks() {
    let base = Timestamp::from_ymd_hms(2026, 11, 17, 10, 0, 0).unwrap();
    assert_eq!(base.timestamp(), 1_794_909_600);
    let later = base + Duration::seconds(90) + Duration::milliseconds(250);
    assert_eq!(later.to_rfc3339_opts(SecondsFormat::Millis, true), "2026-11-17T10:01:30.250Z");
    assert_eq!(later - base, Duration::milliseconds(90_250));
    assert_eq!(base - later, Duration::milliseconds(-90_250));
    assert_eq!(later.signed_duration_since(base), later - base);
    assert_eq!(later - Duration::milliseconds(90_250), base);
    assert_eq!(
        base.checked_add_signed(Duration::days(1)).unwrap().to_rfc3339(),
        "2026-11-18T10:00:00+00:00"
    );
    assert!(base.checked_add_signed(Duration::days(10_000_000)).is_none());
    assert!(base < later);

    // 纳秒计数须能用 i64 表示：约 1677–2262 年。
    assert!(base.timestamp_nanos_opt().is_some());
    assert!(Timestamp::from_ymd_hms(2300, 1, 1, 0, 0, 0).unwrap().timestamp_nanos_opt().is_none());
    assert!(Timestamp::from_ymd_hms(1600, 1, 1, 0, 0, 0).unwrap().timestamp_nanos_opt().is_none());

    assert!(Timestamp::from_ymd_hms(2026, 2, 30, 0, 0, 0).is_none());
    assert!(Timestamp::from_ymd_hms(2026, 1, 1, 24, 0, 0).is_none());
    assert!(Timestamp::from_timestamp(0, 1_000_000_000).is_none());
    assert_eq!(Timestamp::from_timestamp_millis(-1).unwrap().timestamp_subsec_millis(), 999);
    assert_eq!(
        Timestamp::from_timestamp_micros(1_794_909_600_123_456).unwrap().timestamp_subsec_micros(),
        123_456
    );
    assert_eq!(Timestamp::default(), Timestamp::UNIX_EPOCH);
}

#[test]
fn imf_fixdate_accepts_only_the_fixed_width_form_with_a_matching_weekday() {
    let expected = Timestamp::from_timestamp(784_111_777, 0).unwrap();
    assert_eq!(Timestamp::parse_from_imf_fixdate("Sun, 06 Nov 1994 08:49:37 GMT"), Some(expected));
    for rejected in [
        "",
        "Mon, 06 Nov 1994 08:49:37 GMT",
        "sun, 06 Nov 1994 08:49:37 GMT",
        "Sun, 06 nov 1994 08:49:37 GMT",
        "Sun, 6 Nov 1994 08:49:37 GMT",
        "Sun, 06 Nov 1994 08:49:37 UTC",
        "Sun, 06 Nov 1994 08:49:37 +0000",
        "Sun, 06 Nov 1994 08:49:60 GMT",
        "Sun, 06 Nov 1994 24:49:37 GMT",
        "Sun, 31 Nov 1994 08:49:37 GMT",
        "Sun, +6 Nov 1994 08:49:37 GMT",
        "Sunday, 06-Nov-94 08:49:37 GMT",
        "Sun Nov  6 08:49:37 1994",
        "Sun, 06 Nov 1994 08:49:37 GMT ",
        "Sun, 06 Nov 1994 08:49:37 GMＴ",
    ] {
        assert_eq!(Timestamp::parse_from_imf_fixdate(rejected), None, "{rejected:?}");
    }
}

/// 服务端用 strftime 写出的 `Date`/`Last-Modified` 必须能被原样读回。
#[test]
fn imf_fixdate_round_trips_through_strftime() {
    let mut seconds = -62_167_219_200_i64;
    while seconds < 253_370_000_000 {
        let instant = Timestamp::from_timestamp(seconds, 0).unwrap();
        let text = instant.strftime("%a, %d %b %Y %H:%M:%S GMT").to_string();
        assert_eq!(Timestamp::parse_from_imf_fixdate(&text), Some(instant), "{text}");
        seconds += 86_400 * 37 + 3_661;
    }
}
