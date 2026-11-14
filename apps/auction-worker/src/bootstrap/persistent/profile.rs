//! Worker 装配所需的独立投影：拍卖排期运行时配置与数据库连接档位。

use std::sync::Arc;

use auction_application::admin::AdminMailPort;
use auction_config::AppConfig;
use chrono::Timelike;

pub(super) fn configure_schedule_runtime(
    config: &AppConfig,
    mail: Option<Arc<dyn AdminMailPort>>,
) -> anyhow::Result<Option<auction_jobs::AuctionScheduleRuntimeConfig>> {
    let schedule = &config.auction.schedule;
    if !schedule.enabled {
        return Ok(None);
    }
    let parse_day = |value: &str| {
        ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
            .iter()
            .position(|day| *day == value)
            .map(|value| value as u8)
            .ok_or_else(|| anyhow::anyhow!("auction.schedule 星期配置无效"))
    };
    let parse_clock = |value: &str| {
        chrono::NaiveTime::parse_from_str(value, "%H:%M")
            .map(|value| value.hour() as u16 * 60 + value.minute() as u16)
            .map_err(|_| anyhow::anyhow!("auction.schedule 时间配置无效"))
    };
    let runtime_mailboxes = config.worker.mooglemail.configured_mailboxes();
    let worlds = schedule
        .worlds
        .iter()
        .filter(|world_id| {
            runtime_mailboxes
                .clone()
                .any(|(configured_world, _)| configured_world == world_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    anyhow::ensure!(!worlds.is_empty(), "auction.schedule.worlds 与 runtime 启用世界没有交集");
    let mailboxes = runtime_mailboxes
        .map(|(world_id, mailbox_key)| (world_id.to_owned(), mailbox_key.to_owned()))
        .collect();
    // 与生命周期后继共用同一配置投影，保证 bootstrap 与开拍事务计算出同一早鸟时段。
    let early_bird =
        config.auction.schedule_calendar()?.and_then(|calendar| calendar.early_bird().cloned());
    Ok(Some(auction_jobs::AuctionScheduleRuntimeConfig {
        enabled: true,
        timezone: schedule.timezone.clone(),
        preview_weekday: parse_day(&schedule.preview_day)?,
        preview_minute: parse_clock(&schedule.preview_time)?,
        open_weekday: parse_day(&schedule.open_day)?,
        open_minute: parse_clock(&schedule.open_time)?,
        end_weekday: parse_day(&schedule.nominal_end_day)?,
        end_minute: parse_clock(&schedule.nominal_end_time)?,
        worlds,
        policy: config.auction.policy()?,
        early_bird,
        mail,
        mailboxes,
    }))
}

pub(super) fn database_profile(
    config: &auction_config::AppConfig,
) -> Result<auction_storage::DatabaseConnectionProfile, auction_config::ConfigError> {
    use auction_config::{DatabaseProfileMode, Role};
    use auction_storage::{DatabaseConnectionProfile, DatabaseConnectionPurpose as Purpose};
    if config.database_profiles.mode == DatabaseProfileMode::Legacy {
        return Ok(DatabaseConnectionProfile::legacy());
    }
    let purpose = match config.role {
        Role::Public => Purpose::Public,
        Role::Admin => Purpose::Admin,
        Role::Worker => Purpose::Worker,
    };
    let timeouts =
        config.database_pool.timeouts()?.map(|value| auction_storage::DatabaseConnectionTimeouts {
            acquire_millis: value.acquire_timeout_millis,
            statement_millis: value.statement_timeout_millis,
            lock_millis: value.lock_timeout_millis,
            idle_transaction_millis: value.idle_in_transaction_session_timeout_millis,
        });
    Ok(DatabaseConnectionProfile::service(purpose, config.database_pool.max_connections, timeouts))
}
