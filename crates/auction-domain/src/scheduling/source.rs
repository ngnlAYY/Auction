//! 持久化周排期的 IANA 时区与原始本地钟点来源。

use auction_time::{Duration, LocalTime, Timestamp};
use jiff::civil;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use super::local_wire;
use super::wall_clock::parse_timezone;
use crate::DomainError;
use crate::settlement::RECOVERY_V2_PAYMENT_TIMEOUT;

/// 排期来源接受的年份；本地周一最早落在上一年年末，仍不早于公元 0 年。
const SUPPORTED_YEARS: std::ops::RangeInclusive<i16> = 1..=9999;

/// 一轮排期冻结的本地计划；保留 DST 跳时归一化前的钟点，后继不从 UTC 反推配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuctionScheduleSource {
    pub timezone: String,
    #[serde(with = "local_wire")]
    pub preview_local: civil::DateTime,
    #[serde(with = "local_wire")]
    pub open_local: civil::DateTime,
    #[serde(with = "local_wire")]
    pub end_local: civil::DateTime,
    /// 赛季早鸟场次的稳定身份；常规周轮次省略，保持升级前快照与请求摘要的序列化不变。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub early_bird: Option<AuctionEarlyBirdSlot>,
}

/// 赛季早鸟场次身份：赛季开始的本地日期与场次序号（0 为赛季前场次，1 起为窗口内场次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuctionEarlyBirdSlot {
    pub season: civil::Date,
    pub session: u32,
}

impl AuctionEarlyBirdSlot {
    /// 是否为赛季前场次（序号 0）；它冻结 24 小时付款期限的恢复策略。
    pub fn is_pre_season(&self) -> bool {
        self.session == 0
    }

    /// 轮次唯一键与稳定 UUID 的身份片段，格式 `early-bird:YYYY-MM-DD:N`。
    pub fn key(&self) -> String {
        format!("early-bird:{}:{}", self.season.strftime("%Y-%m-%d"), self.session)
    }
}

/// 排期来源解析出的三个 UTC 时间；仅供首次物化和后继生成，不覆盖已冻结轮次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuctionScheduleTimes {
    pub preview_due_at: Timestamp,
    pub open_due_at: Timestamp,
    pub scheduled_end_at: Timestamp,
}

impl AuctionScheduleSource {
    /// 复用旧周计划的歧义时间取较早实例、跳时时间按后续 offset 归一化语义。
    pub fn resolve(&self) -> Result<AuctionScheduleTimes, DomainError> {
        self.ensure_supported_years()?;
        let timezone = parse_timezone(&self.timezone)
            .ok_or(DomainError::InvalidSchedule("周排期时区来源无效"))?;
        let result = AuctionScheduleTimes {
            preview_due_at: resolve_local(&timezone, self.preview_local)?,
            open_due_at: resolve_local(&timezone, self.open_local)?,
            scheduled_end_at: resolve_local(&timezone, self.end_local)?,
        };
        if result.preview_due_at >= result.open_due_at
            || result.open_due_at >= result.scheduled_end_at
        {
            return Err(DomainError::InvalidSchedule("周排期时间来源未严格递增"));
        }
        Ok(result)
    }

    /// 按本地日历推进七天，使跨 DST 的后继保持同一墙上钟点。
    /// 早鸟场次不是周循环，拒绝推进，后继由排期日历另行计算。
    pub fn next_week(&self) -> Result<Self, DomainError> {
        if self.early_bird.is_some() {
            return Err(DomainError::InvalidSchedule("早鸟场次不能按周推进"));
        }
        self.shifted_local_days(7)
    }

    /// 保留原时区与本地钟点整体顺延若干天，并去除早鸟身份；仅用于没有日历时的兜底后继。
    pub fn shifted_local_days(&self, days: i64) -> Result<Self, DomainError> {
        // 越界的来源不能靠顺延回到支持范围里继续使用；其余合法性仍只校验顺延后的结果。
        self.ensure_supported_years()?;
        let shift = Duration::try_days(days)
            .ok_or(DomainError::InvalidSchedule("下一期本地时间超出范围"))?;
        let next = |value: civil::DateTime| {
            value
                .checked_add(shift.to_jiff())
                .ok()
                .ok_or(DomainError::InvalidSchedule("下一期本地时间超出范围"))
        };
        let source = Self {
            timezone: self.timezone.clone(),
            preview_local: next(self.preview_local)?,
            open_local: next(self.open_local)?,
            end_local: next(self.end_local)?,
            early_bird: None,
        };
        source.resolve()?;
        Ok(source)
    }

    /// 日期限定在公元 1–9999 年：这之外的年份，日期与身份文本的写法不再与已持久化的格式一致
    /// （负年份的位数不同），失败关闭而不是换一种写法继续。
    fn ensure_supported_years(&self) -> Result<(), DomainError> {
        let supported = SUPPORTED_YEARS.contains(&self.preview_local.year())
            && SUPPORTED_YEARS.contains(&self.open_local.year())
            && SUPPORTED_YEARS.contains(&self.end_local.year())
            && self.early_bird.is_none_or(|slot| SUPPORTED_YEARS.contains(&slot.season.year()));
        supported.ok_or(DomainError::InvalidSchedule("周排期日期超出支持范围"))
    }

    /// Admin 改期只更新实际改变的钟点；不把未改的跳时原始钟点替换成归一化后的时间。
    pub fn with_updated_times(
        &self,
        previous: AuctionScheduleTimes,
        updated: AuctionScheduleTimes,
    ) -> Result<Self, DomainError> {
        self.resolve()?;
        let timezone = parse_timezone(&self.timezone)
            .ok_or(DomainError::InvalidSchedule("周排期时区来源无效"))?;
        let mut source = self.clone();
        if previous.preview_due_at != updated.preview_due_at {
            source.preview_local = timezone.to_datetime(updated.preview_due_at.to_jiff());
        }
        if previous.open_due_at != updated.open_due_at {
            source.open_local = timezone.to_datetime(updated.open_due_at.to_jiff());
        }
        if previous.scheduled_end_at != updated.scheduled_end_at {
            source.end_local = timezone.to_datetime(updated.scheduled_end_at.to_jiff());
        }
        let resolved = source.resolve()?;
        // 折叠小时里同一本地钟点对应两个 UTC 瞬间；反解再解析固定取较早实例，
        // 会让改期偏离命令给出的 UTC。只要求实际改动的时刻保持原值。
        let keeps_requested_instant = [
            (previous.preview_due_at, updated.preview_due_at, resolved.preview_due_at),
            (previous.open_due_at, updated.open_due_at, resolved.open_due_at),
            (previous.scheduled_end_at, updated.scheduled_end_at, resolved.scheduled_end_at),
        ]
        .into_iter()
        .all(|(previous, updated, resolved)| previous == updated || resolved == updated);
        if !keeps_requested_instant {
            return Err(DomainError::InvalidSchedule("改期时刻无法由本地钟点唯一表示"));
        }
        // 改期不得保留旧赛季身份却把付款窗口推过换季；使用命令的实际 UTC 截止。
        validate_pre_season_payment_deadline(
            &timezone,
            source.early_bird,
            updated.scheduled_end_at,
        )?;
        Ok(source)
    }
}

/// 新场次物化和 Admin 改期共用付款边界；读取冻结来源本身不改变其历史解释。
pub(super) fn validate_pre_season_payment_deadline(
    timezone: &TimeZone,
    early_bird: Option<AuctionEarlyBirdSlot>,
    scheduled_end_at: Timestamp,
) -> Result<(), DomainError> {
    let Some(slot) = early_bird.filter(AuctionEarlyBirdSlot::is_pre_season) else {
        return Ok(());
    };
    // 付款期限是固定的 UTC 时长；DST 前跳可能使本地分钟校验通过的期限跨过换季边界。
    let payment_due_at = scheduled_end_at
        .checked_add_signed(RECOVERY_V2_PAYMENT_TIMEOUT)
        .ok_or(DomainError::InvalidSchedule("赛季前付款截止时间超出范围"))?;
    let reset_at = resolve_local(timezone, slot.season.to_datetime(civil::Time::midnight()))?;
    if payment_due_at >= reset_at {
        return Err(DomainError::InvalidSchedule("赛季前付款截止不得到达换季日"));
    }
    Ok(())
}

/// 本地墙上时间转 UTC；排期日历与冻结来源共用同一 DST 规则。
///
/// 重复出现的墙上时间取较早的一次；不存在的墙上时间沿用 Go `time.Date` 的既有兼容规则，
/// 按跳时之后的 offset 归一化。
pub(super) fn resolve_local(
    timezone: &TimeZone,
    local: civil::DateTime,
) -> Result<Timestamp, DomainError> {
    match LocalTime::resolve(timezone, local) {
        LocalTime::Unique(instant) | LocalTime::Repeated { earlier: instant, .. } => Ok(instant),
        LocalTime::Skipped { after } => after
            .to_timestamp(local)
            .map(Timestamp::from)
            .ok()
            .ok_or(DomainError::InvalidSchedule("周排期 UTC 时间超出范围")),
    }
}
