//! 拍卖经济、结算和恢复策略校验。

use chrono::Timelike;

use super::super::*;

pub(crate) fn validate_auction(config: &AuctionConfig) -> Result<(), ConfigError> {
    if !matches!(config.settlement.mode.as_str(), "legacy_zero_v0" | "hold_curve_v1") {
        return Err(ConfigError::InvalidAuctionConfig(
            "settlement.mode 只能是 legacy_zero_v0 或 hold_curve_v1",
        ));
    }
    if config.settlement.recovery.mode != "remind_v1" {
        return Err(ConfigError::InvalidAuctionConfig("settlement.recovery.mode 只能是 remind_v1"));
    }
    if config.schedule.enabled {
        validate_schedule(&config.schedule)?;
    }
    config
        .policy()?
        .validate()
        .map_err(|_| ConfigError::InvalidAuctionConfig("经济、结算或恢复字段边界无效"))
}

fn validate_schedule(schedule: &AuctionScheduleConfig) -> Result<(), ConfigError> {
    if schedule.timezone.parse::<chrono_tz::Tz>().is_err()
        || schedule.worlds.is_empty()
        || schedule.worlds.len() > 2
        || schedule.worlds.iter().any(|world| !matches!(world.as_str(), "main" | "isekai"))
        || schedule.worlds.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(ConfigError::InvalidAuctionConfig("auction.schedule 字段无效"));
    }
    let clock = |day: &str, time: &str| {
        let day = u32::try_from(
            ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
                .iter()
                .position(|value| *value == day)?,
        )
        .ok()?;
        let parsed = chrono::NaiveTime::parse_from_str(time, "%H:%M").ok()?;
        Some((day, parsed.hour() * 60 + parsed.minute()))
    };
    let Some(preview) = clock(&schedule.preview_day, &schedule.preview_time) else {
        return Err(ConfigError::InvalidAuctionConfig("auction.schedule preview 时间无效"));
    };
    let Some(open) = clock(&schedule.open_day, &schedule.open_time) else {
        return Err(ConfigError::InvalidAuctionConfig("auction.schedule open 时间无效"));
    };
    let Some(end) = clock(&schedule.nominal_end_day, &schedule.nominal_end_time) else {
        return Err(ConfigError::InvalidAuctionConfig("auction.schedule nominal_end 时间无效"));
    };
    if preview.0 * 24 * 60 + preview.1 >= open.0 * 24 * 60 + open.1
        || open.0 * 24 * 60 + open.1 >= end.0 * 24 * 60 + end.1
        || open.0 * 24 * 60 + open.1 - (preview.0 * 24 * 60 + preview.1) < 120
    {
        return Err(ConfigError::InvalidAuctionConfig("auction.schedule 时间顺序或暂停窗口无效"));
    }
    Ok(())
}
