//! 宽松文本解析的接受范围回归；期望取自迁移前的历史实现。

use jiff::civil;

use super::parse_clock;
use super::parse_date;
use super::parse_minute_datetime;

#[test]
fn clock_accepts_unpadded_fields_and_leading_whitespace() {
    assert_eq!(parse_clock("18:00"), Some(civil::time(18, 0, 0, 0)));
    assert_eq!(parse_clock("9:05"), Some(civil::time(9, 5, 0, 0)));
    assert_eq!(parse_clock(" 9: 5"), Some(civil::time(9, 5, 0, 0)));
    for rejected in ["", "24:00", "23:60", "9:05 ", "09:00:00", "+9:05", "9-05", "123:00", "9:"] {
        assert_eq!(parse_clock(rejected), None, "{rejected:?}");
    }
}

#[test]
fn date_accepts_unpadded_fields_and_signed_years() {
    assert_eq!(parse_date("2026-11-05"), Some(civil::date(2026, 11, 5)));
    assert_eq!(parse_date("2026-1-5"), Some(civil::date(2026, 1, 5)));
    assert_eq!(parse_date(" 2026- 1- 5"), Some(civil::date(2026, 1, 5)));
    assert_eq!(parse_date("+2026-01-05"), Some(civil::date(2026, 1, 5)));
    assert_eq!(parse_date("-0001-01-05"), Some(civil::date(-1, 1, 5)));
    assert_eq!(parse_date("0000-02-29"), Some(civil::date(0, 2, 29)));
    for rejected in [
        "",
        "2026-02-30",
        "2026-13-01",
        "2026-00-10",
        "2026-11-05 ",
        "20261-01-01",
        "+ 2026-01-05",
        "2026/11/05",
        "2026-11",
        "+10000-01-01",
    ] {
        assert_eq!(parse_date(rejected), None, "{rejected:?}");
    }
}

#[test]
fn minute_datetime_allows_any_whitespace_between_date_and_clock() {
    let expected = Some(civil::datetime(2026, 10, 18, 5, 14, 0, 0));
    assert_eq!(parse_minute_datetime("2026-10-18 05:14"), expected);
    assert_eq!(parse_minute_datetime("2026-10-18   5:14"), expected);
    assert_eq!(parse_minute_datetime("2026-10-1805:14"), expected);
    for rejected in ["2026-10-18", "2026-10-18 05:14:00", "2026-10-18T05:14", "2026-10-18 24:00"] {
        assert_eq!(parse_minute_datetime(rejected), None, "{rejected:?}");
    }
}
