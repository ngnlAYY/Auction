//! HTTP `Date` 头的 IMF-fixdate 形式（RFC 9110 §5.6.7）：`Sun, 06 Nov 1994 08:49:37 GMT`。
//!
//! 只接受发送方必须生成的这一种定宽、区分大小写的写法，星期须与日期相符。浏览器端用它读取
//! 响应的 `Date` 头校准时钟，不为此引入通用的 strptime 解析。

use super::Timestamp;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const IMF_FIXDATE_LENGTH: usize = 29;

impl Timestamp {
    /// 解析 IMF-fixdate；形状、取值范围或星期与日期不符时返回 `None`。
    pub fn parse_from_imf_fixdate(text: &str) -> Option<Self> {
        if text.len() != IMF_FIXDATE_LENGTH {
            return None;
        }
        let part = |start: usize, end: usize| text.get(start..end);
        let number = |start: usize, end: usize| {
            let digits = part(start, end)?;
            digits.bytes().all(|byte| byte.is_ascii_digit()).then(|| digits.parse::<u32>().ok())?
        };
        let separators = part(3, 5) == Some(", ")
            && part(7, 8) == Some(" ")
            && part(11, 12) == Some(" ")
            && part(16, 17) == Some(" ")
            && part(19, 20) == Some(":")
            && part(22, 23) == Some(":")
            && part(25, 29) == Some(" GMT");
        if !separators {
            return None;
        }
        let weekday = WEEKDAYS.iter().position(|name| part(0, 3) == Some(*name))?;
        let month = MONTHS.iter().position(|name| part(8, 11) == Some(*name))?.checked_add(1)?;
        let parsed = Self::from_ymd_hms(
            i32::try_from(number(12, 16)?).ok()?,
            u32::try_from(month).ok()?,
            number(5, 7)?,
            number(17, 19)?,
            number(20, 22)?,
            number(23, 25)?,
        )?;
        let actual = parsed.to_utc_datetime().weekday().to_monday_zero_offset();
        (usize::try_from(actual).ok() == Some(weekday)).then_some(parsed)
    }
}
