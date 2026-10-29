//! 拍卖经济、结算和恢复策略校验。

use chrono::Timelike;

use super::super::*;

pub(crate) fn validate_auction(config: &AuctionConfig) -> Result<(), ConfigError> {
    let policy = config.policy()?;
    if config.schedule.enabled {
        validate_schedule(&config.schedule)?;
    }
    validate_early_bird(&config.schedule)?;
    policy.validate().map_err(|_| ConfigError::InvalidAuctionConfig("经济、结算或恢复字段边界无效"))
}

impl AuctionScheduleConfig {
    /// 校验周计划与早鸟期，即使暂未启用也可检查候选；首次设置与正式 loader 共用同一规则。
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_schedule(self)?;
        validate_early_bird(self)
    }
}

fn validate_early_bird(schedule: &AuctionScheduleConfig) -> Result<(), ConfigError> {
    // 早鸟期即使未启用也完整校验；启用时必须依附已启用的周排期及其世界。
    schedule
        .early_bird
        .validate()
        .map_err(|_| ConfigError::InvalidAuctionConfig("auction.schedule.early_bird 字段无效"))?;
    if schedule.early_bird.enabled
        && (!schedule.enabled
            || schedule.early_bird.worlds.iter().any(|world| !schedule.worlds.contains(world)))
    {
        return Err(ConfigError::InvalidAuctionConfig(
            "auction.schedule.early_bird 启用时须启用 auction.schedule，且早鸟世界须包含在排期世界内",
        ));
    }
    Ok(())
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
