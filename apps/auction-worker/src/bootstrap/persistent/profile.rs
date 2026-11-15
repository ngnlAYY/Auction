//! Worker 装配所需的独立投影：拍卖排期运行时配置与数据库连接档位。

use std::sync::Arc;

use auction_application::admin::AdminMailPort;
use auction_config::AppConfig;

pub(super) fn configure_schedule_runtime(
    config: &AppConfig,
    mail: Option<Arc<dyn AdminMailPort>>,
) -> anyhow::Result<Option<auction_jobs::AuctionScheduleRuntimeConfig>> {
    let schedule = &config.auction.schedule;
    if !schedule.enabled {
        return Ok(None);
    }
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
    // 周计划与早鸟日历都取自生命周期后继使用的同一日历投影（领域解析器是唯一 owner），
    // 保证 bootstrap 与开拍事务计算出同一周时段与早鸟时段，不在装配根另写一份解析。
    let calendar = config
        .auction
        .schedule_calendar()?
        .ok_or_else(|| anyhow::anyhow!("auction.schedule 已启用但未生成排期日历"))?;
    Ok(Some(auction_jobs::AuctionScheduleRuntimeConfig {
        enabled: true,
        weekly: calendar.weekly().clone(),
        worlds,
        policy: config.auction.policy()?,
        early_bird: calendar.early_bird().cloned(),
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::path::Path;

    use auction_config::{
        AppConfig, AuctionScheduleConfig, FirstRunConfig, PostgresConnectionConfig, Role,
    };
    use chrono::{DateTime, NaiveDate, Utc};
    use uuid::Uuid;

    use super::configure_schedule_runtime;

    /// 用首次设置生成完整配置目录；只物化临时文件，不连接数据库或外部服务。
    fn write_config(destination: &Path) {
        FirstRunConfig {
            database: PostgresConnectionConfig {
                host: "127.0.0.1".to_owned(),
                port: 5432,
                username: "auction_runtime".to_owned(),
                password: "not-a-real-database-password".to_owned(),
                name: "auction_schedule_projection".to_owned(),
            },
            admin_actor: "operator".to_owned(),
            admin_secret: "a".repeat(32),
            public_origin: "https://auction.example.com".to_owned(),
            admin_origin: "https://admin.example.com".to_owned(),
            public_listener_port: 8090,
            admin_listener_port: 8091,
            metrics_enabled: false,
            schedule: AuctionScheduleConfig {
                enabled: true,
                timezone: "America/New_York".to_owned(),
                preview_day: "Saturday".to_owned(),
                preview_time: "23:00".to_owned(),
                open_day: "Sunday".to_owned(),
                // 2026-03-08 夏令时开始当天没有 02:30，两条投影须按同一跳时规则归一化。
                open_time: "02:30".to_owned(),
                nominal_end_day: "Sunday".to_owned(),
                nominal_end_time: "12:00".to_owned(),
                ..AuctionScheduleConfig::default()
            },
            start_price: 1000,
            log_filter: "info".to_owned(),
            worker_concurrency: 4,
            worker_poll_interval_seconds: 15,
            realtime_poll_interval_millis: 250,
            control_group: "auction".to_owned(),
            control_uid: 1000,
        }
        .write_to(destination)
        .unwrap();
    }

    /// Worker 运行期配置直接采用配置层排期日历（领域解析器是唯一 owner），并按冻结来源的 DST
    /// 规则给出时段：2026-03-08 纽约跳时当天不存在的 02:30 按跳时后的 offset（EDT）归一化。
    fn assert_schedule(config: &AppConfig, preview_due_at: &str) {
        let calendar = config.auction.schedule_calendar().unwrap().unwrap();
        let runtime = configure_schedule_runtime(config, None).unwrap().unwrap();
        assert_eq!(&runtime.weekly, calendar.weekly());
        assert_eq!(runtime.early_bird.as_ref(), calendar.early_bird());
        let week = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
        let times = runtime.weekly.slot(week).unwrap().times;
        let utc = |value: &str| value.parse::<DateTime<Utc>>().unwrap();
        assert_eq!(times.preview_due_at, utc(preview_due_at));
        assert_eq!(times.open_due_at, utc("2026-03-08T06:30:00Z"));
        assert_eq!(times.scheduled_end_at, utc("2026-03-08T16:00:00Z"));
    }

    #[test]
    fn worker_schedule_runtime_matches_config_calendar_slots() {
        let directory =
            std::env::temp_dir().join(format!("auction-worker-schedule-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let config_dir = directory.join("config");
        write_config(&config_dir);
        let worker_path = config_dir.join("worker.toml");
        let config = AppConfig::load_from_path(Role::Worker, &worker_path).unwrap();
        assert_schedule(&config, "2026-03-08T04:00:00Z");

        // 正式 loader 沿用 Chrono 的宽松钟点（如 "9:00"）；Worker 不得另用更严或更松的解析。
        let common_path = config_dir.join("common.toml");
        let common = fs::read_to_string(&common_path).unwrap();
        let relaxed = common.replace("preview_time = \"23:00\"", "preview_time = \"9:00\"");
        assert_ne!(relaxed, common);
        fs::write(&common_path, relaxed).unwrap();
        let relaxed_config = AppConfig::load_from_path(Role::Worker, &worker_path).unwrap();
        assert_eq!(relaxed_config.auction.schedule.preview_time, "9:00");
        assert_schedule(&relaxed_config, "2026-03-07T14:00:00Z");
        fs::remove_dir_all(directory).unwrap();
    }
}
