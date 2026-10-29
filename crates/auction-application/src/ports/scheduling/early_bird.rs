//! 赛季早鸟期配置与日历：窗口、场次时段、常规周场替代及收件品质时间窗。
//! 只做纯计算，不访问数据库或读取进程配置；common.toml 与 Admin runtime 共用同一 DTO 和校验，
//! 避免两处维护不同的时序规则。

use auction_domain::{DomainError, valid_text};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::schedule_calendar::AuctionScheduleSlot;
use super::schedule_source::{
    AuctionEarlyBirdSlot, AuctionScheduleSource, AuctionScheduleTimes, resolve_local,
};

/// 早鸟窗口与场次日的最大天数。
pub const MAX_EARLY_BIRD_DAYS: u32 = 31;
/// 预览提前量下限；与 Worker 开拍前两小时暂停收件邮箱的门禁一致。
pub const MIN_EARLY_BIRD_PREVIEW_LEAD_MINUTES: u32 = 120;
/// 预览提前量与竞价时长的上限（七天）。
pub const MAX_EARLY_BIRD_MINUTES: u32 = 7 * 24 * 60;

const MINUTES_PER_DAY: i64 = 24 * 60;

/// 赛季早鸟期配置；所有字段可省略，未启用时完整校验但不改变排期、后继与收件品质。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionEarlyBirdConfig {
    /// 是否启用早鸟期；默认关闭，并且仅在周排期启用时生效。
    pub enabled: bool,
    /// 赛季开始与场次钟点使用的 IANA 时区，默认 UTC。
    pub timezone: String,
    /// 参与早鸟期的世界，默认只有 isekai。
    pub worlds: Vec<String>,
    /// 每年赛季开始的月-日（MM-DD），默认 05-01、11-01；不接受 02-29。
    pub season_starts: Vec<String>,
    /// 赛季开始的本地钟点（HH:MM），默认 09:00；早鸟窗口与收件品质从此刻起算。
    pub season_start_time: String,
    /// 早鸟窗口天数，默认 7。
    pub window_days: u32,
    /// 窗口内开拍的第几天（1 起始、严格递增），默认 [1, 3, 5, 7]。
    pub session_days: Vec<u32>,
    /// 每个场次日的本地开拍钟点（HH:MM），默认 18:00。
    pub open_time: String,
    /// 预览早于开拍的分钟数，默认 120；不得小于 120。
    pub preview_lead_minutes: u32,
    /// 开拍到计划结束的分钟数，默认 1440。
    pub live_minutes: u32,
    /// 窗口内收到的投稿接受的品质；空数组表示不限品质，默认空。
    pub qualities: Vec<String>,
    /// 首场早鸟草稿建立后至赛季开始前是否暂停该世界的收件轮询，默认 true。
    pub pause_intake_before_season: bool,
}

impl Default for AuctionEarlyBirdConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timezone: "UTC".to_owned(),
            worlds: vec!["isekai".to_owned()],
            season_starts: vec!["05-01".to_owned(), "11-01".to_owned()],
            season_start_time: "09:00".to_owned(),
            window_days: 7,
            session_days: vec![1, 3, 5, 7],
            open_time: "18:00".to_owned(),
            preview_lead_minutes: 120,
            live_minutes: 24 * 60,
            qualities: Vec::new(),
            pause_intake_before_season: true,
        }
    }
}

impl AuctionEarlyBirdConfig {
    /// 校验字段边界与跨字段时序；失败返回首个无效字段名，供配置与 runtime 错误定位。
    pub fn validate(&self) -> Result<(), &'static str> {
        self.parse().map(|_| ())
    }

    /// 已启用时返回日历；未启用时仍先完整校验，保证保存的候选随时可以启用。
    pub fn active_calendar(&self) -> Result<Option<AuctionEarlyBirdCalendar>, &'static str> {
        let calendar = self.parse()?;
        Ok(self.enabled.then_some(calendar))
    }

    fn parse(&self) -> Result<AuctionEarlyBirdCalendar, &'static str> {
        if self.timezone.len() > 128 {
            return Err("timezone");
        }
        let timezone: Tz = self.timezone.parse().map_err(|_| "timezone")?;
        if self.worlds.is_empty()
            || self.worlds.len() > 2
            || self.worlds.iter().any(|world| !matches!(world.as_str(), "main" | "isekai"))
            || self.worlds.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err("worlds");
        }
        let season_starts = parse_season_starts(&self.season_starts).ok_or("season_starts")?;
        let season_start = parse_clock(&self.season_start_time).ok_or("season_start_time")?;
        if !(1..=MAX_EARLY_BIRD_DAYS).contains(&self.window_days) {
            return Err("window_days");
        }
        if self.session_days.is_empty()
            || self.session_days.len() > MAX_EARLY_BIRD_DAYS as usize
            || self.session_days.iter().any(|day| !(1..=self.window_days).contains(day))
            || self.session_days.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err("session_days");
        }
        let open_time = parse_clock(&self.open_time).ok_or("open_time")?;
        if !(MIN_EARLY_BIRD_PREVIEW_LEAD_MINUTES..=MAX_EARLY_BIRD_MINUTES)
            .contains(&self.preview_lead_minutes)
        {
            return Err("preview_lead_minutes");
        }
        if !(1..=MAX_EARLY_BIRD_MINUTES).contains(&self.live_minutes) {
            return Err("live_minutes");
        }
        validate_qualities(&self.qualities).ok_or("qualities")?;

        // 时序按本地分钟校验；DST 造成的个别小时偏差由开拍前的同世界竞价门禁兜底。
        let lead = i64::from(self.preview_lead_minutes);
        let live = i64::from(self.live_minutes);
        let start_minute = clock_minute(season_start);
        let open_minute = clock_minute(open_time);
        let open_offset =
            |day: u32| (i64::from(day) - 1) * MINUTES_PER_DAY + open_minute - start_minute;
        // 首场预览不得早于赛季开始，否则早鸟场次会在旧赛季内开放预览。
        if open_offset(self.session_days[0]) - lead < 0 {
            return Err("session_days");
        }
        // 相邻场次的下一场预览不得早于上一场计划结束，避免同一世界计划内重叠竞价。
        if self
            .session_days
            .windows(2)
            .any(|pair| (i64::from(pair[1]) - i64::from(pair[0])) * MINUTES_PER_DAY - lead < live)
        {
            return Err("live_minutes");
        }
        let last_day = self.session_days[self.session_days.len() - 1];
        let coverage =
            (i64::from(self.window_days) * MINUTES_PER_DAY).max(open_offset(last_day) + live);
        validate_season_gaps(&season_starts, coverage).ok_or("season_starts")?;

        Ok(AuctionEarlyBirdCalendar {
            timezone_name: self.timezone.clone(),
            timezone,
            worlds: self.worlds.clone(),
            season_starts,
            season_start,
            window_days: self.window_days,
            session_days: self.session_days.clone(),
            open_time,
            preview_lead: Duration::minutes(lead),
            live: Duration::minutes(live),
            qualities: self.qualities.clone(),
            pause_intake_before_season: self.pause_intake_before_season,
        })
    }
}

/// 一个赛季的早鸟窗口及其覆盖期（窗口结束与末场计划结束中较晚者）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuctionEarlyBirdSeason {
    /// 赛季开始的本地日期，也是早鸟场次身份的一部分。
    pub season: NaiveDate,
    pub window_start: DateTime<Utc>,
    pub window_end: DateTime<Utc>,
    pub coverage_end: DateTime<Utc>,
}

/// 已校验且启用的早鸟日历。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionEarlyBirdCalendar {
    timezone_name: String,
    timezone: Tz,
    worlds: Vec<String>,
    season_starts: Vec<(u32, u32)>,
    season_start: NaiveTime,
    window_days: u32,
    session_days: Vec<u32>,
    open_time: NaiveTime,
    preview_lead: Duration,
    live: Duration,
    qualities: Vec<String>,
    pause_intake_before_season: bool,
}

impl AuctionEarlyBirdCalendar {
    /// 该世界是否参与早鸟期。
    pub fn applies_to(&self, world_id: &str) -> bool {
        self.worlds.iter().any(|world| world == world_id)
    }

    /// 早鸟窗口内的接收品质；空切片表示不限品质。
    pub fn qualities(&self) -> &[String] {
        &self.qualities
    }

    /// 赛季开始前是否暂停首场早鸟草稿所在世界的收件轮询。
    pub fn pauses_intake_before_season(&self) -> bool {
        self.pause_intake_before_season
    }

    /// 按 1 起始序号计算某赛季的早鸟场次时段。
    pub fn session(
        &self,
        season: NaiveDate,
        session: u32,
    ) -> Result<AuctionScheduleSlot, DomainError> {
        let index = usize::try_from(session)
            .ok()
            .and_then(|session| session.checked_sub(1))
            .ok_or(DomainError::InvalidSchedule("早鸟场次序号无效"))?;
        let day = *self
            .session_days
            .get(index)
            .ok_or(DomainError::InvalidSchedule("早鸟场次序号无效"))?;
        let open_date = season
            .checked_add_signed(Duration::days(i64::from(day) - 1))
            .ok_or(DomainError::InvalidSchedule("早鸟场次日期超出范围"))?;
        let open_local = open_date.and_time(self.open_time);
        let preview_local = open_local
            .checked_sub_signed(self.preview_lead)
            .ok_or(DomainError::InvalidSchedule("早鸟预览时间超出范围"))?;
        let end_local = open_local
            .checked_add_signed(self.live)
            .ok_or(DomainError::InvalidSchedule("早鸟结束时间超出范围"))?;
        let source = AuctionScheduleSource {
            timezone: self.timezone_name.clone(),
            preview_local,
            open_local,
            end_local,
            early_bird: Some(AuctionEarlyBirdSlot { season, session }),
        };
        let times = source.resolve()?;
        let local_week =
            open_date - Duration::days(i64::from(open_date.weekday().num_days_from_monday()));
        Ok(AuctionScheduleSlot { source, times, local_week })
    }

    /// 返回早鸟场次所属赛季。
    pub fn season_of(
        &self,
        slot: AuctionEarlyBirdSlot,
    ) -> Result<AuctionEarlyBirdSeason, DomainError> {
        self.season(slot.season)
    }

    /// 某赛季的全部早鸟场次，按开拍顺序排列。
    pub fn sessions(
        &self,
        season: &AuctionEarlyBirdSeason,
    ) -> Result<Vec<AuctionScheduleSlot>, DomainError> {
        (1..=self.session_count()).map(|session| self.session(season.season, session)).collect()
    }

    /// `at` 是否位于该世界某赛季的早鸟窗口 `[开始, 开始 + window_days)` 内。
    pub fn window_contains(&self, world_id: &str, at: DateTime<Utc>) -> bool {
        self.applies_to(world_id)
            && self
                .seasons_around(at)
                .any(|season| season.window_start <= at && at < season.window_end)
    }

    /// 常规周时段 `[预览, 计划结束)` 与早鸟覆盖期 `[窗口开始, 覆盖结束)` 重叠时由早鸟场次替代。
    pub fn suppresses(&self, world_id: &str, times: &AuctionScheduleTimes) -> bool {
        self.applies_to(world_id)
            && self.seasons_around(times.preview_due_at).any(|season| overlaps(&season, times))
    }

    /// 预览不早于 `floor` 的最早早鸟场次；世界不参与或附近没有赛季时返回 `None`。
    pub fn first_session_from(
        &self,
        world_id: &str,
        floor: DateTime<Utc>,
    ) -> Result<Option<AuctionScheduleSlot>, DomainError> {
        if !self.applies_to(world_id) {
            return Ok(None);
        }
        for season in self.seasons_around(floor) {
            if season.coverage_end <= floor {
                continue;
            }
            for slot in self.sessions(&season)? {
                if slot.times.preview_due_at >= floor {
                    return Ok(Some(slot));
                }
            }
        }
        Ok(None)
    }

    /// 周时段被替代时应维护的早鸟场次：与该周时段重叠的赛季中最早一个尚未结束的场次。
    pub fn current_session(
        &self,
        world_id: &str,
        weekly: &AuctionScheduleTimes,
        now: DateTime<Utc>,
    ) -> Result<Option<AuctionScheduleSlot>, DomainError> {
        if !self.applies_to(world_id) {
            return Ok(None);
        }
        for season in self.seasons_around(weekly.preview_due_at) {
            if !overlaps(&season, weekly) {
                continue;
            }
            for slot in self.sessions(&season)? {
                if slot.times.scheduled_end_at > now {
                    return Ok(Some(slot));
                }
            }
        }
        Ok(None)
    }

    /// 窗口开始晚于 `now` 的最近赛季。
    pub fn upcoming_season(
        &self,
        world_id: &str,
        now: DateTime<Utc>,
    ) -> Option<AuctionEarlyBirdSeason> {
        if !self.applies_to(world_id) {
            return None;
        }
        self.seasons_around(now).find(|season| season.window_start > now)
    }

    fn session_count(&self) -> u32 {
        u32::try_from(self.session_days.len()).unwrap_or(u32::MAX)
    }

    fn season(&self, season: NaiveDate) -> Result<AuctionEarlyBirdSeason, DomainError> {
        let start_local = season.and_time(self.season_start);
        let window_start = resolve_local(self.timezone, start_local)?;
        let end_local = start_local
            .checked_add_signed(Duration::days(i64::from(self.window_days)))
            .ok_or(DomainError::InvalidSchedule("早鸟窗口结束时间超出范围"))?;
        let window_end = resolve_local(self.timezone, end_local)?;
        // 收件窗口与覆盖期只依赖赛季边界和末场结束时间；某场预览/开拍碰到 DST
        // 非法顺序时，session() 仍拒绝物化，但不能把整个赛季误判为普通收件期。
        let last_day =
            self.session_days.last().ok_or(DomainError::InvalidSchedule("早鸟场次为空"))?;
        let last_end_local = season
            .and_time(self.open_time)
            .checked_add_signed(Duration::days(i64::from(*last_day) - 1))
            .and_then(|open| open.checked_add_signed(self.live))
            .ok_or(DomainError::InvalidSchedule("早鸟结束时间超出范围"))?;
        let last_end = resolve_local(self.timezone, last_end_local)?;
        Ok(AuctionEarlyBirdSeason {
            season,
            window_start,
            window_end,
            coverage_end: window_end.max(last_end),
        })
    }

    /// 按本地年份和已排序月日逐个生成前后各一年的赛季，无法表示的日期仍跳过。
    /// 查询首个命中后停止，不为每封收件或每个周候选分配并排序完整赛季列表。
    fn seasons_around(
        &self,
        at: DateTime<Utc>,
    ) -> impl Iterator<Item = AuctionEarlyBirdSeason> + '_ {
        let year = at.with_timezone(&self.timezone).year();
        (year.saturating_sub(1)..=year.saturating_add(1)).flat_map(move |candidate_year| {
            self.season_starts.iter().filter_map(move |(month, day)| {
                let date = NaiveDate::from_ymd_opt(candidate_year, *month, *day)?;
                self.season(date).ok()
            })
        })
    }
}

fn overlaps(season: &AuctionEarlyBirdSeason, times: &AuctionScheduleTimes) -> bool {
    times.preview_due_at < season.coverage_end && season.window_start < times.scheduled_end_at
}

fn parse_clock(value: &str) -> Option<NaiveTime> {
    if value.len() != 5 {
        return None;
    }
    NaiveTime::parse_from_str(value, "%H:%M").ok()
}

fn clock_minute(value: NaiveTime) -> i64 {
    i64::from(value.hour()) * 60 + i64::from(value.minute())
}

/// 解析 MM-DD；以平年校验日期，使每个配置日期在任意年份都存在。
fn parse_season_starts(values: &[String]) -> Option<Vec<(u32, u32)>> {
    if values.is_empty() || values.len() > 12 {
        return None;
    }
    let mut parsed = Vec::with_capacity(values.len());
    for value in values {
        let bytes = value.as_bytes();
        if bytes.len() != 5
            || bytes[2] != b'-'
            || !bytes[..2].iter().chain(&bytes[3..]).all(u8::is_ascii_digit)
        {
            return None;
        }
        let month = value.get(0..2)?.parse::<u32>().ok()?;
        let day = value.get(3..5)?.parse::<u32>().ok()?;
        NaiveDate::from_ymd_opt(2001, month, day)?;
        if parsed.contains(&(month, day)) {
            return None;
        }
        parsed.push((month, day));
    }
    parsed.sort_unstable();
    Some(parsed)
}

/// 相邻赛季（含跨年）的开始间隔必须容纳前一赛季的完整覆盖期。
fn validate_season_gaps(season_starts: &[(u32, u32)], coverage_minutes: i64) -> Option<()> {
    let ordinals = season_starts
        .iter()
        .map(|(month, day)| NaiveDate::from_ymd_opt(2001, *month, *day).map(|date| date.ordinal()))
        .collect::<Option<Vec<_>>>()?;
    for (index, ordinal) in ordinals.iter().enumerate() {
        let next = ordinals.get(index + 1).copied().unwrap_or(ordinals[0] + 365);
        if i64::from(next - ordinal) * MINUTES_PER_DAY < coverage_minutes {
            return None;
        }
    }
    Some(())
}

fn validate_qualities(values: &[String]) -> Option<()> {
    if values.len() > 256 {
        return None;
    }
    for (index, value) in values.iter().enumerate() {
        if !valid_text(value, 128)
            || values[..index].iter().any(|previous| previous.eq_ignore_ascii_case(value))
        {
            return None;
        }
    }
    Some(())
}
