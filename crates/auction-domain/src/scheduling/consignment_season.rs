//! 寄售季窗：从日历 owner 的时区与换季日期换算赛季开始、结束和预停截止。
//!
//! 换季日与提前 N 个**本地日历日**的本地 00:00 分别换算 UTC，不使用早鸟开场时刻，也不使用
//! `end − N×86400`。冻结版本涵盖世界、时区、换季日期与 N；调用方不能靠推迟日历延长授权。
//! 季前（`at < start`）保持 `unknown`：不自动关闭、不自动处置，挂单保持
//! `held_off_market/review`；这是 2026-11-21 统筹方裁定的本期合同。

use std::fmt;

use auction_time::Timestamp;
use jiff::ToSpan;
use jiff::civil;
use jiff::tz::TimeZone;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::early_bird::AuctionEarlyBirdConfig;
use super::source::resolve_local;
use super::wall_clock::parse_timezone;
use crate::SettlementWorld;
use crate::delivery::settlement_world_code;

/// 预停下限：最多提前 90 个本地日历日。
pub const MAX_PRECLOSE_DAYS: u8 = 90;

/// 季窗在计算、冻结与判定时使用的状态闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeasonState {
    /// main 世界没有寄售季界。
    NotSeasonal,
    /// 可签发新预约、换发和购买。
    Open,
    /// 到达预停点，禁新授权但原有效预约仍可在原截止前匹配。
    Preclose,
    /// 旧季已关闭。
    Closed,
    /// 边界无法判定，失败关闭。
    Unknown,
}

impl SeasonState {
    /// 全部状态；协议闭集必须逐项对应。
    pub const ALL: [Self; 5] =
        [Self::NotSeasonal, Self::Open, Self::Preclose, Self::Closed, Self::Unknown];

    /// 稳定 wire 与数据库码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotSeasonal => "not_seasonal",
            Self::Open => "open",
            Self::Preclose => "preclose",
            Self::Closed => "closed",
            Self::Unknown => "unknown",
        }
    }

    /// 闭集解析，未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }
}

/// 日历输入的规范摘要。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CalendarVersion([u8; 32]);

impl CalendarVersion {
    /// 从已冻结的 32 字节摘要构造，供存储适配器与黄金向量使用。
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// 返回固定 32 字节摘要。
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for CalendarVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CalendarVersion(")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        formatter.write_str(")")
    }
}

/// 一个已冻结或当前已知的寄售季窗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenSeason {
    /// 世界；main 不会产生冻结季窗。
    pub world: SettlementWorld,
    /// 赛季本地开始日期 `YYYY-MM-DD`，作为稳定季身份。
    pub season_id: String,
    /// 赛季开始 UTC。
    pub start_utc: Timestamp,
    /// 下一赛季开始 UTC，即旧季结束。
    pub end_utc: Timestamp,
    /// 预停 UTC；到达等号即禁新授权。
    pub admission_cutoff_utc: Timestamp,
    /// 含世界、时区、换季日期与 N 的规范版本。
    pub calendar_version: CalendarVersion,
    /// 生成该窗口时使用的提前本地日历日数。
    pub preclose_days: u8,
}

/// 季窗输入或边界错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SeasonError {
    /// main 世界没有季窗。
    #[error("main 世界没有寄售季窗")]
    NotSeasonal,
    /// 时区名不是内嵌 IANA 数据库的精确写法。
    #[error("寄售日历时区无效")]
    InvalidTimezone,
    /// 换季日期不是规范的 `MM-DD`，或重复、为空。
    #[error("寄售换季日期无效")]
    InvalidSeasonStarts,
    /// 提前天数不在 1..=90。
    #[error("寄售预停天数无效")]
    InvalidPrecloseDays,
    /// 无法从输入年份附近生成完整的前后季窗。
    #[error("寄售季窗无法表示")]
    Unrepresentable,
    /// 边界没有严格有序，或预停早于赛季开始。
    #[error("寄售季窗边界无效")]
    InvalidWindow,
}

/// 由现有日历 owner 的时区与换季日期构造的寄售日历。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentCalendar {
    world: SettlementWorld,
    timezone_name: String,
    timezone: TimeZone,
    season_starts: Vec<(i8, i8)>,
}

impl ConsignmentCalendar {
    /// 从世界、IANA 时区与规范 `MM-DD` 列表构造；日期排序去重后冻结。
    pub fn new(
        world: SettlementWorld,
        timezone: &str,
        season_starts: &[String],
    ) -> Result<Self, SeasonError> {
        let timezone = parse_timezone(timezone).ok_or(SeasonError::InvalidTimezone)?;
        if world == SettlementWorld::Main {
            return Err(SeasonError::NotSeasonal);
        }
        let season_starts = parse_season_starts(season_starts)?;
        let timezone_name = timezone.iana_name().ok_or(SeasonError::InvalidTimezone)?.to_owned();
        Ok(Self { world, timezone_name, timezone, season_starts })
    }

    /// 复用既有早鸟日历配置的时区与换季日期；不读取早鸟开场钟点或窗口天数。
    pub fn from_early_bird_config(config: &AuctionEarlyBirdConfig) -> Result<Self, SeasonError> {
        Self::new(SettlementWorld::Isekai, &config.timezone, &config.season_starts)
    }

    /// 返回该日历对应的世界。
    pub const fn world(&self) -> SettlementWorld {
        self.world
    }

    /// 返回规范日历版本；N 参与摘要。
    pub fn calendar_version(&self, preclose_days: u8) -> Result<CalendarVersion, SeasonError> {
        if !(1..=MAX_PRECLOSE_DAYS).contains(&preclose_days) {
            return Err(SeasonError::InvalidPrecloseDays);
        }
        let mut input = b"auction.consignment.season-calendar.v1\0".to_vec();
        write_text(&mut input, &self.timezone_name);
        write_text(&mut input, settlement_world_code(self.world));
        input.extend_from_slice(
            &u64::try_from(self.season_starts.len())
                .map_err(|_| SeasonError::InvalidSeasonStarts)?
                .to_be_bytes(),
        );
        for (month, day) in &self.season_starts {
            write_text(&mut input, &format!("{month:02}-{day:02}"));
        }
        input.extend_from_slice(&u64::from(preclose_days).to_be_bytes());
        let digest = Sha256::digest(input);
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&digest);
        Ok(CalendarVersion(bytes))
    }

    /// 计算 `at` 所在季窗及其预停截止。
    ///
    /// 不变量 6、10：换季日与提前本地日历日的 00:00 分别换算 UTC；冻结后不能用新的日历延长。
    pub fn season_window(
        &self,
        at: Timestamp,
        preclose_days: u8,
    ) -> Result<FrozenSeason, SeasonError> {
        if self.world == SettlementWorld::Main {
            return Err(SeasonError::NotSeasonal);
        }
        if !(1..=MAX_PRECLOSE_DAYS).contains(&preclose_days) {
            return Err(SeasonError::InvalidPrecloseDays);
        }
        let local_year = self.timezone.to_datetime(at.to_jiff()).year();
        let mut starts = Vec::new();
        for year in local_year.saturating_sub(1)..=local_year.saturating_add(1) {
            for (month, day) in &self.season_starts {
                let date = civil::Date::new(year, *month, *day)
                    .map_err(|_| SeasonError::InvalidSeasonStarts)?;
                let instant =
                    resolve_local(&self.timezone, date.to_datetime(civil::Time::midnight()))
                        .map_err(|_| SeasonError::Unrepresentable)?;
                starts.push((date, instant));
            }
        }
        starts.sort_by_key(|(_, instant)| *instant);

        let Some(index) = starts.iter().rposition(|(_, instant)| *instant <= at) else {
            return Err(SeasonError::Unrepresentable);
        };
        let (season_date, start_utc) = starts[index];
        let (next_date, end_utc) =
            starts.get(index + 1).copied().ok_or(SeasonError::Unrepresentable)?;
        let cutoff_date = next_date
            .checked_sub(i64::from(preclose_days).days())
            .map_err(|_| SeasonError::InvalidWindow)?;
        let admission_cutoff_utc =
            resolve_local(&self.timezone, cutoff_date.to_datetime(civil::Time::midnight()))
                .map_err(|_| SeasonError::Unrepresentable)?;
        if start_utc >= end_utc
            || admission_cutoff_utc < start_utc
            || admission_cutoff_utc >= end_utc
        {
            return Err(SeasonError::InvalidWindow);
        }
        Ok(FrozenSeason {
            world: self.world,
            season_id: season_date.strftime("%Y-%m-%d").to_string(),
            start_utc,
            end_utc,
            admission_cutoff_utc,
            calendar_version: self.calendar_version(preclose_days)?,
            preclose_days,
        })
    }
}

/// 判定季态；冻结界与当前已知界取更严者。
///
/// `end` 与 `cutoff` 取较早值，`start` 取较晚值。任一输入缺失时不猜测：main 返回
/// `not_seasonal`，isekai 两侧都缺失返回 `unknown`。
///
/// 季前（`at < start`）同样返回 `unknown`，不自动关闭或处置；调用方不得据此授权，
/// 挂单保持 `held_off_market/review`。这是 2026-11-21 统筹方裁定的本期合同。
pub fn season_state(
    world: SettlementWorld,
    frozen: Option<&FrozenSeason>,
    current: Option<&FrozenSeason>,
    at: Timestamp,
) -> SeasonState {
    if world == SettlementWorld::Main {
        return SeasonState::NotSeasonal;
    }
    let windows = [frozen, current]
        .into_iter()
        .flatten()
        .filter(|window| window.world == SettlementWorld::Isekai)
        .collect::<Vec<_>>();
    if windows.is_empty() {
        return SeasonState::Unknown;
    }
    let start = windows.iter().map(|window| window.start_utc).max();
    let end = windows.iter().map(|window| window.end_utc).min();
    let cutoff = windows.iter().map(|window| window.admission_cutoff_utc).min();
    let (Some(start), Some(end), Some(cutoff)) = (start, end, cutoff) else {
        return SeasonState::Unknown;
    };
    if start >= end || cutoff >= end {
        return SeasonState::Unknown;
    }
    if at < start {
        return SeasonState::Unknown;
    }
    if at >= end {
        return SeasonState::Closed;
    }
    if at >= cutoff {
        return SeasonState::Preclose;
    }
    SeasonState::Open
}

fn parse_season_starts(values: &[String]) -> Result<Vec<(i8, i8)>, SeasonError> {
    if values.is_empty() || values.len() > 12 {
        return Err(SeasonError::InvalidSeasonStarts);
    }
    let mut parsed = Vec::with_capacity(values.len());
    for value in values {
        let bytes = value.as_bytes();
        if bytes.len() != 5
            || bytes[2] != b'-'
            || !bytes[..2].iter().chain(&bytes[3..]).all(u8::is_ascii_digit)
        {
            return Err(SeasonError::InvalidSeasonStarts);
        }
        let month = value
            .get(0..2)
            .and_then(|value| value.parse::<i8>().ok())
            .ok_or(SeasonError::InvalidSeasonStarts)?;
        let day = value
            .get(3..5)
            .and_then(|value| value.parse::<i8>().ok())
            .ok_or(SeasonError::InvalidSeasonStarts)?;
        civil::Date::new(2001, month, day).map_err(|_| SeasonError::InvalidSeasonStarts)?;
        if parsed.contains(&(month, day)) {
            return Err(SeasonError::InvalidSeasonStarts);
        }
        parsed.push((month, day));
    }
    parsed.sort_unstable();
    Ok(parsed)
}

fn write_text(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};

    use super::{ConsignmentCalendar, SeasonError, SeasonState, season_state};
    use crate::SettlementWorld;

    fn calendar(starts: &[&str]) -> ConsignmentCalendar {
        ConsignmentCalendar::new(
            SettlementWorld::Isekai,
            "America/Los_Angeles",
            &starts.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>(),
        )
        .expect("测试日历")
    }

    #[test]
    fn main_has_no_season_state() {
        assert_eq!(
            season_state(SettlementWorld::Main, None, None, Timestamp::UNIX_EPOCH),
            SeasonState::NotSeasonal
        );
    }

    #[test]
    fn dst_fall_back_local_midnights_differ_by_25_hours() {
        let window = calendar(&["11-01", "11-02"])
            .season_window(Timestamp::from_ymd_hms(2026, 11, 1, 20, 0, 0).expect("测试时刻"), 1)
            .expect("秋回拨窗口");
        assert_eq!(window.end_utc.signed_duration_since(window.start_utc), Duration::hours(25));
    }

    #[test]
    fn dst_spring_forward_local_midnights_differ_by_23_hours() {
        let window = calendar(&["03-08", "03-09"])
            .season_window(Timestamp::from_ymd_hms(2026, 3, 8, 20, 0, 0).expect("测试时刻"), 1)
            .expect("春拨快窗口");
        assert_eq!(window.end_utc.signed_duration_since(window.start_utc), Duration::hours(23));
    }

    #[test]
    fn cutoff_equality_is_preclose_and_end_equality_is_closed() {
        let calendar = calendar(&["05-01", "11-01"]);
        let window = calendar
            .season_window(Timestamp::from_ymd_hms(2026, 6, 1, 0, 0, 0).expect("测试时刻"), 3)
            .expect("季窗");
        let before = window.admission_cutoff_utc - Duration::nanoseconds(1);
        assert_eq!(
            season_state(SettlementWorld::Isekai, Some(&window), Some(&window), before),
            SeasonState::Open
        );
        assert_eq!(
            season_state(
                SettlementWorld::Isekai,
                Some(&window),
                Some(&window),
                window.admission_cutoff_utc
            ),
            SeasonState::Preclose
        );
        assert_eq!(
            season_state(SettlementWorld::Isekai, Some(&window), Some(&window), window.end_utc),
            SeasonState::Closed
        );
    }

    #[test]
    fn delayed_current_calendar_cannot_extend_frozen_boundary() {
        let calendar = calendar(&["05-01", "11-01"]);
        let frozen = calendar
            .season_window(Timestamp::from_ymd_hms(2026, 6, 1, 0, 0, 0).expect("测试时刻"), 3)
            .expect("冻结季窗");
        let mut delayed = frozen.clone();
        delayed.end_utc = frozen.end_utc + Duration::days(1);
        delayed.admission_cutoff_utc = frozen.admission_cutoff_utc + Duration::days(1);
        assert_eq!(
            season_state(SettlementWorld::Isekai, Some(&frozen), Some(&delayed), frozen.end_utc),
            SeasonState::Closed
        );
    }

    #[test]
    fn missing_windows_are_unknown() {
        assert_eq!(
            season_state(SettlementWorld::Isekai, None, None, Timestamp::UNIX_EPOCH),
            SeasonState::Unknown
        );
    }

    #[test]
    fn pre_season_boundary_stays_unknown() {
        let calendar = calendar(&["05-01", "11-01"]);
        let window = calendar
            .season_window(Timestamp::from_ymd_hms(2026, 6, 1, 0, 0, 0).expect("测试时刻"), 3)
            .expect("季窗");
        assert_eq!(
            season_state(
                SettlementWorld::Isekai,
                Some(&window),
                Some(&window),
                window.start_utc - Duration::nanoseconds(1)
            ),
            SeasonState::Unknown
        );
    }

    #[test]
    fn calendar_version_has_a_frozen_golden_vector() {
        let version = calendar(&["05-01", "11-01"]).calendar_version(3).expect("日历版本");
        let expected = [
            0x6e, 0x4d, 0x9f, 0xe8, 0x1e, 0xd9, 0x8f, 0x0a, 0x0b, 0xe6, 0x2c, 0xa6, 0x21, 0xf2,
            0xd7, 0xb3, 0xaa, 0x3a, 0x66, 0x69, 0xd8, 0x15, 0xf2, 0xae, 0x00, 0xfe, 0x53, 0xcc,
            0x1f, 0xd9, 0x64, 0xa5,
        ];
        assert_eq!(version.as_bytes(), expected);
        assert_eq!(
            calendar(&["05-01", "11-01"]).calendar_version(4).map(super::CalendarVersion::as_bytes),
            Ok([
                0xc0, 0x77, 0x59, 0xa5, 0x7c, 0x63, 0x76, 0xc1, 0x4e, 0xd5, 0x08, 0x3c, 0x19, 0xd7,
                0x32, 0xc5, 0xfb, 0x64, 0x38, 0x42, 0x23, 0xe3, 0x0f, 0x88, 0xf0, 0xab, 0xd4, 0x16,
                0xcb, 0xd1, 0x06, 0x67,
            ])
        );
    }

    #[test]
    fn main_calendar_and_invalid_inputs_are_rejected() {
        assert_eq!(
            ConsignmentCalendar::new(
                SettlementWorld::Main,
                "America/Los_Angeles",
                &["05-01".to_owned()]
            ),
            Err(SeasonError::NotSeasonal)
        );
        for starts in [
            vec![],
            vec!["5-01".to_owned()],
            vec!["05-1".to_owned()],
            vec!["05-01".to_owned(), "05-01".to_owned()],
            vec!["02-29".to_owned()],
        ] {
            assert_eq!(
                ConsignmentCalendar::new(SettlementWorld::Isekai, "America/Los_Angeles", &starts),
                Err(SeasonError::InvalidSeasonStarts)
            );
        }
    }
}
