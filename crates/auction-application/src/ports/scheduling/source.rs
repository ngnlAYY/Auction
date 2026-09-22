//! 持久化周排期的 IANA 时区与原始本地钟点来源。

use auction_domain::DomainError;
use chrono::{DateTime, Duration, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// 一轮排期冻结的本地计划；保留 DST 跳时归一化前的钟点，后继不从 UTC 反推配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuctionScheduleSource {
    pub timezone: String,
    pub preview_local: NaiveDateTime,
    pub open_local: NaiveDateTime,
    pub end_local: NaiveDateTime,
}

/// 排期来源解析出的三个 UTC 时间；仅供首次物化和后继生成，不覆盖已冻结轮次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuctionScheduleTimes {
    pub preview_due_at: DateTime<Utc>,
    pub open_due_at: DateTime<Utc>,
    pub scheduled_end_at: DateTime<Utc>,
}

impl AuctionScheduleSource {
    /// 复用旧周计划的歧义时间取较早实例、跳时时间按后续 offset 归一化语义。
    pub fn resolve(&self) -> Result<AuctionScheduleTimes, DomainError> {
        let timezone: Tz = self
            .timezone
            .parse()
            .map_err(|_| DomainError::InvalidSchedule("周排期时区来源无效"))?;
        let result = AuctionScheduleTimes {
            preview_due_at: resolve_local(timezone, self.preview_local)?,
            open_due_at: resolve_local(timezone, self.open_local)?,
            scheduled_end_at: resolve_local(timezone, self.end_local)?,
        };
        if result.preview_due_at >= result.open_due_at
            || result.open_due_at >= result.scheduled_end_at
        {
            return Err(DomainError::InvalidSchedule("周排期时间来源未严格递增"));
        }
        Ok(result)
    }

    /// 按本地日历推进七天，使跨 DST 的后继保持同一墙上钟点。
    pub fn next_week(&self) -> Result<Self, DomainError> {
        let next = |value: NaiveDateTime| {
            value
                .checked_add_signed(Duration::days(7))
                .ok_or(DomainError::InvalidSchedule("下一期本地时间超出范围"))
        };
        let source = Self {
            timezone: self.timezone.clone(),
            preview_local: next(self.preview_local)?,
            open_local: next(self.open_local)?,
            end_local: next(self.end_local)?,
        };
        source.resolve()?;
        Ok(source)
    }

    /// Admin 改期只更新实际改变的钟点；不把未改的跳时原始钟点替换成归一化后的时间。
    pub fn with_updated_times(
        &self,
        previous: AuctionScheduleTimes,
        updated: AuctionScheduleTimes,
    ) -> Result<Self, DomainError> {
        self.resolve()?;
        let timezone: Tz = self
            .timezone
            .parse()
            .map_err(|_| DomainError::InvalidSchedule("周排期时区来源无效"))?;
        let mut source = self.clone();
        if previous.preview_due_at != updated.preview_due_at {
            source.preview_local = updated.preview_due_at.with_timezone(&timezone).naive_local();
        }
        if previous.open_due_at != updated.open_due_at {
            source.open_local = updated.open_due_at.with_timezone(&timezone).naive_local();
        }
        if previous.scheduled_end_at != updated.scheduled_end_at {
            source.end_local = updated.scheduled_end_at.with_timezone(&timezone).naive_local();
        }
        source.resolve()?;
        Ok(source)
    }
}

fn resolve_local(timezone: Tz, local: NaiveDateTime) -> Result<DateTime<Utc>, DomainError> {
    if let Some(value) = timezone.from_local_datetime(&local).earliest() {
        return Ok(value.with_timezone(&Utc));
    }
    // Go time.Date 的既有兼容规则：不存在的墙上时间使用跳时后的 offset 归一化。
    let offset = (1_i64..=48 * 60).find_map(|minutes| {
        local
            .checked_add_signed(Duration::minutes(minutes))
            .and_then(|candidate| timezone.offset_from_local_datetime(&candidate).earliest())
    });
    let offset_seconds = i64::from(
        offset
            .ok_or(DomainError::InvalidSchedule("周排期本地时间无法解析"))?
            .fix()
            .local_minus_utc(),
    );
    let utc = local
        .checked_sub_signed(Duration::seconds(offset_seconds))
        .ok_or(DomainError::InvalidSchedule("周排期 UTC 时间超出范围"))?;
    Ok(DateTime::from_naive_utc_and_offset(utc, Utc))
}
