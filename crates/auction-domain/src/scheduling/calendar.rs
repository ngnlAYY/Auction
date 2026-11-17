//! 周排期与赛季早鸟组合日历：为 Worker bootstrap 和开拍后继计算可物化的计划时段。
//! 已冻结的轮次事实不由这里改写；日历只决定尚未建立的下一轮。

use auction_time::Timestamp;
use jiff::ToSpan;
use jiff::civil;
use jiff::tz::TimeZone;

use super::early_bird::AuctionEarlyBirdCalendar;
use super::source::{AuctionScheduleSource, AuctionScheduleTimes};
use super::wall_clock::{parse_clock_minute, parse_timezone};
use crate::DomainError;

/// 周时段向后搜索的上限，覆盖约两年；超过即视为配置无法产生合法时段。
const MAX_WEEKLY_SEARCH: i64 = 106;

const WEEKDAYS: [&str; 7] =
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// 一个可物化为拍卖轮次的计划时段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionScheduleSlot {
    pub source: AuctionScheduleSource,
    pub times: AuctionScheduleTimes,
    /// 时段所在本地周的周一；常规周轮次以它作为 world 内唯一身份。
    pub local_week: civil::Date,
}

impl AuctionScheduleSlot {
    /// 早鸟时段的轮次唯一键；常规周时段返回 `None`。
    pub fn slot_key(&self) -> Option<String> {
        self.source.early_bird.map(|slot| slot.key())
    }

    /// 轮次、拍卖与命令稳定 UUID 的身份片段：早鸟用时段键，常规周沿用 `YYYY-MM-DD`。
    pub fn identity_key(&self) -> String {
        self.slot_key().unwrap_or_else(|| self.local_week.strftime("%Y-%m-%d").to_string())
    }
}

/// 已校验的本地周计划；星期编号周一为 0。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionWeeklySchedule {
    timezone_name: String,
    timezone: TimeZone,
    preview: (u8, u16),
    open: (u8, u16),
    end: (u8, u16),
}

impl AuctionWeeklySchedule {
    /// 使用星期编号与当地分钟建立周计划；要求同一周内 `预览 + 120 <= 开拍 < 结束`。
    pub fn new(
        timezone: &str,
        preview: (u8, u16),
        open: (u8, u16),
        end: (u8, u16),
    ) -> Result<Self, DomainError> {
        let parsed =
            parse_timezone(timezone).ok_or(DomainError::InvalidSchedule("周排期时区无效"))?;
        let week_minute = |(day, minute): (u8, u16)| {
            (day <= 6 && minute < 24 * 60)
                .then(|| u32::from(day) * 24 * 60 + u32::from(minute))
                .ok_or(DomainError::InvalidSchedule("周排期星期或钟点无效"))
        };
        let (preview_at, open_at, end_at) =
            (week_minute(preview)?, week_minute(open)?, week_minute(end)?);
        if preview_at + 120 > open_at || open_at >= end_at {
            return Err(DomainError::InvalidSchedule("周排期时间顺序或暂停窗口无效"));
        }
        Ok(Self { timezone_name: timezone.to_owned(), timezone: parsed, preview, open, end })
    }

    /// 使用英文星期全名与 `HH:MM` 建立周计划，供配置层投影。
    pub fn from_names(
        timezone: &str,
        preview: (&str, &str),
        open: (&str, &str),
        end: (&str, &str),
    ) -> Result<Self, DomainError> {
        let parse = |(day, time): (&str, &str)| {
            let day = WEEKDAYS
                .iter()
                .position(|candidate| *candidate == day)
                .and_then(|day| u8::try_from(day).ok())
                .ok_or(DomainError::InvalidSchedule("周排期星期无效"))?;
            // 部署配置沿用既有的钟点解析范围，投影不得另加补零限制。
            let minute =
                parse_clock_minute(time).ok_or(DomainError::InvalidSchedule("周排期钟点无效"))?;
            Ok::<_, DomainError>((day, minute))
        };
        Self::new(timezone, parse(preview)?, parse(open)?, parse(end)?)
    }

    /// 返回 `at` 所在本地周的周一。
    pub fn week_start(&self, at: Timestamp) -> civil::Date {
        let local = self.timezone.to_datetime(at.to_jiff()).date();
        local - i64::from(local.weekday().to_monday_zero_offset()).days()
    }

    /// 计算以 `week_start` 为周一的周时段，复用冻结来源的 DST 解析规则。
    pub fn slot(&self, week_start: civil::Date) -> Result<AuctionScheduleSlot, DomainError> {
        let local_at = |(day, minute): (u8, u16)| {
            let date = week_start
                .checked_add(i64::from(day).days())
                .ok()
                .ok_or(DomainError::InvalidSchedule("周排期日期超出范围"))?;
            let time = i8::try_from(minute / 60)
                .ok()
                .zip(i8::try_from(minute % 60).ok())
                .and_then(|(hour, minute)| civil::Time::new(hour, minute, 0, 0).ok())
                .ok_or(DomainError::InvalidSchedule("周排期钟点无效"))?;
            Ok::<_, DomainError>(date.to_datetime(time))
        };
        let source = AuctionScheduleSource {
            timezone: self.timezone_name.clone(),
            preview_local: local_at(self.preview)?,
            open_local: local_at(self.open)?,
            end_local: local_at(self.end)?,
            early_bird: None,
        };
        let times = source.resolve()?;
        Ok(AuctionScheduleSlot { source, times, local_week: week_start })
    }

    /// 从 `floor` 所在周起，返回预览不早于 `floor` 且未被早鸟替代的第一个周时段。
    pub fn first_slot_from(
        &self,
        floor: Timestamp,
        suppressed: impl Fn(&AuctionScheduleTimes) -> bool,
    ) -> Result<AuctionScheduleSlot, DomainError> {
        let first_week = self.week_start(floor);
        for offset in 0..MAX_WEEKLY_SEARCH {
            let week = first_week
                .checked_add((offset * 7).days())
                .ok()
                .ok_or(DomainError::InvalidSchedule("周排期日期超出范围"))?;
            let slot = self.slot(week)?;
            if slot.times.preview_due_at >= floor && !suppressed(&slot.times) {
                return Ok(slot);
            }
        }
        Err(DomainError::InvalidSchedule("找不到未被早鸟替代的周时段"))
    }
}

/// 周排期启用时的组合日历；早鸟日历仅在早鸟期启用时存在。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionScheduleCalendar {
    weekly: AuctionWeeklySchedule,
    early_bird: Option<AuctionEarlyBirdCalendar>,
}

impl AuctionScheduleCalendar {
    pub fn new(
        weekly: AuctionWeeklySchedule,
        early_bird: Option<AuctionEarlyBirdCalendar>,
    ) -> Self {
        Self { weekly, early_bird }
    }

    pub fn weekly(&self) -> &AuctionWeeklySchedule {
        &self.weekly
    }

    /// 返回参与该世界的早鸟日历；未启用或世界不在范围内时为 `None`。
    pub fn early_bird_for(&self, world_id: &str) -> Option<&AuctionEarlyBirdCalendar> {
        self.early_bird.as_ref().filter(|calendar| calendar.applies_to(world_id))
    }

    /// 早鸟日历本身，供收件品质等跨世界判定复用。
    pub fn early_bird(&self) -> Option<&AuctionEarlyBirdCalendar> {
        self.early_bird.as_ref()
    }
}
