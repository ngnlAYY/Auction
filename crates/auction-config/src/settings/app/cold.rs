//! Worker 启动业务配置的规范投影。版本 1 的字段与序列化形态固定。

use auction_application::admin::WorkerColdConfigDigest;
use auction_application::forum::EquipmentColorRule;
use auction_application::mail::default_accepted_qualities;
use auction_application::sent_baseline::SentBaselineMode;
use auction_domain::equipment::taxonomy::armor_material_category;
use auction_domain::{AuctionEarlyBirdConfig, parse_clock_minute};
use serde::Serialize;

use super::accessors::{DEFAULT_WORKER_CONCURRENCY, DEFAULT_WORKER_POLL_INTERVAL_SECONDS};
use super::model::AppConfig;
use crate::settings::worker_validation::forum::parse_public_base_url;
use crate::settings::{
    AuctionConfig, ConfigError, ForumSyncWorkerConfig, FulfillmentWorkerConfig,
    MoogleMailEquipmentFilterConfig, MoogleMailWorkerConfig, OperationsAlertWorkerConfig,
    RecoveryWorkerConfig, Role, WorkerConfig,
};

const VERSION: u32 = 1;

impl AppConfig {
    /// 从已装配的 Worker 启动配置计算非敏感摘要。
    pub fn worker_cold_config_digest(&self) -> Result<WorkerColdConfigDigest, ConfigError> {
        if self.role != Role::Worker {
            return Err(ConfigError::InvalidRuntimeWorkerConfig("冷配置摘要必须来自 Worker 装配"));
        }
        digest(&self.auction, &self.worker, &self.bot_players)
    }
}

fn digest(
    auction: &AuctionConfig,
    worker: &WorkerConfig,
    bot_players: &[String],
) -> Result<WorkerColdConfigDigest, ConfigError> {
    // 只接收显式选定的已解析业务字段，通道地址只取通过校验的无凭据 origin。
    #[derive(Serialize)]
    struct Projection<E, S, M, F, A, L> {
        version: u32,
        economy: E,
        schedule: S,
        mail: M,
        forum: F,
        public_base_origin: Option<String>,
        alerts: A,
        loops: L,
        bot_players: Vec<String>,
    }
    let projection = Projection {
        version: VERSION,
        economy: (&auction.economics, &auction.settlement),
        schedule: schedule(auction, &worker.mooglemail)?,
        mail: mail(&worker.mooglemail)?,
        forum: forum(&worker.forum)?,
        // 被超提醒也使用公开地址，论坛同步关闭时仍然生效。
        public_base_origin: worker
            .forum
            .public_base_url
            .as_deref()
            .map(|value| parse_public_base_url(value).map(|url| url.origin().ascii_serialization()))
            .transpose()?,
        alerts: alerts(&worker.operations_alert),
        loops: loops(worker),
        bot_players: ascii_set(bot_players),
    };
    WorkerColdConfigDigest::from_projection(VERSION, &projection)
        .map_err(|_error| ConfigError::InvalidRuntimeWorkerConfig("Worker 冷配置摘要序列化失败"))
}

fn schedule<'a>(
    auction: &'a AuctionConfig,
    mail: &'a MoogleMailWorkerConfig,
) -> Result<Option<impl Serialize + 'a>, ConfigError> {
    #[derive(Serialize)]
    struct Schedule<'a> {
        timezone: &'a str,
        preview: (&'a str, u16),
        open: (&'a str, u16),
        end: (&'a str, u16),
        worlds_and_mailboxes: Vec<(String, &'a str)>,
        early_bird: Option<AuctionEarlyBirdConfig>,
    }
    let config = &auction.schedule;
    let Some(worlds) = config.worker_worlds(mail)? else {
        return Ok(None);
    };
    let minute = |clock: &str| {
        parse_clock_minute(clock)
            .ok_or(ConfigError::InvalidAuctionConfig("auction.schedule 钟点无效"))
    };
    let early_bird = if config.early_bird.enabled {
        let mut early = config.early_bird.clone();
        early.worlds.sort_unstable();
        early.season_starts.sort_unstable();
        early.qualities = ascii_set(&early.qualities);
        let clock = |value: &str| {
            let minute = minute(value)?;
            Ok::<_, ConfigError>(format!("{:02}:{:02}", minute / 60, minute % 60))
        };
        early.season_start_time = clock(&early.season_start_time)?;
        early.open_time = clock(&early.open_time)?;
        if !early.pre_season_session {
            early.pre_season_days_before = AuctionEarlyBirdConfig::default().pre_season_days_before;
        }
        Some(early)
    } else {
        None
    };
    // 排期扫描按有效世界次序运行；无交集的世界不会进入调度。
    let worlds_and_mailboxes = worlds
        .into_iter()
        .map(|world| {
            let (_, key) = mail
                .configured_mailboxes()
                .find(|(configured, _)| *configured == world)
                .ok_or(ConfigError::InvalidAuctionConfig("排期世界缺少邮箱标识"))?;
            Ok((world, key))
        })
        .collect::<Result<_, ConfigError>>()?;
    Ok(Some(Schedule {
        timezone: &config.timezone,
        preview: (&config.preview_day, minute(&config.preview_time)?),
        open: (&config.open_day, minute(&config.open_time)?),
        end: (&config.nominal_end_day, minute(&config.nominal_end_time)?),
        worlds_and_mailboxes,
        early_bird,
    }))
}

fn mail(config: &MoogleMailWorkerConfig) -> Result<Option<impl Serialize>, ConfigError> {
    #[derive(Serialize)]
    struct Mail<C, F> {
        endpoint_origin: String,
        worlds: Vec<MailWorld>,
        receive_schedule: C,
        maintenance_schedule: C,
        equipment_filter: F,
        start_price: i64,
        rekey_enabled: bool,
        maximum_response_bytes: usize,
        minimum_request_interval_millis: u64,
        request_timeout_seconds: u64,
        maximum_pages: usize,
        sent_baseline_mode: &'static str,
        poll_isolation_enabled: bool,
        read_mail_observation_interval_seconds: u64,
        mail_scan_daily_full_interval_seconds: u64,
        lease_duration_seconds: u64,
        renew_interval_seconds: u64,
    }
    #[derive(Serialize)]
    struct MailWorld {
        world_id: String,
        mailbox_key: String,
        sender_allowlist: Option<Vec<String>>,
    }
    if !config.enabled {
        return Ok(None);
    }
    let receive = config.receive_schedule()?;
    let maintenance = config
        .maintenance_schedule
        .as_deref()
        .map(crate::StandardCronSchedule::parse)
        .transpose()
        .map_err(|_error| ConfigError::InvalidMoogleMailConfig("维护日程无效"))?;
    let sent_baseline_mode = match config.sent_baseline_mode {
        SentBaselineMode::Full => "full",
        SentBaselineMode::Shadow => "shadow",
        SentBaselineMode::Watermark => "watermark",
    };
    Ok(Some(Mail {
        endpoint_origin: config.endpoint_url()?.origin().ascii_serialization(),
        worlds: config
            .configured_worlds()
            .into_iter()
            .map(|world| MailWorld {
                world_id: world.world_id,
                mailbox_key: world.mailbox_key,
                sender_allowlist: (!world.sender_allowlist_disabled)
                    .then(|| ascii_set(&world.sender_allowlist)),
            })
            .collect(),
        receive_schedule: receive.cold_projection(),
        maintenance_schedule: maintenance.as_ref().unwrap_or(&receive).cold_projection(),
        equipment_filter: equipment_filter(&config.equipment_filter),
        start_price: config.start_price.unwrap_or(50_000),
        rekey_enabled: config.rekey_enabled,
        maximum_response_bytes: config.maximum_response_bytes,
        minimum_request_interval_millis: config.minimum_request_interval_millis,
        request_timeout_seconds: config.request_timeout_seconds,
        maximum_pages: config.maximum_pages,
        sent_baseline_mode,
        poll_isolation_enabled: config.poll_isolation_enabled,
        read_mail_observation_interval_seconds: config.read_mail_observation_interval_seconds,
        mail_scan_daily_full_interval_seconds: config.mail_scan_daily_full_interval_seconds,
        lease_duration_seconds: config.lease_duration_seconds,
        renew_interval_seconds: config.renew_interval_seconds,
    }))
}

fn equipment_filter(config: &MoogleMailEquipmentFilterConfig) -> impl Serialize {
    #[derive(Serialize)]
    struct Filters<'a> {
        prefixes: Vec<&'a str>,
        suffixes: Vec<&'a str>,
        categories: Vec<&'a str>,
        equipment_types: Vec<&'a str>,
        level_min: i32,
        level_max: i32,
    }
    // 品质始终参与接收；其余过滤器只在启用时参与。
    let qualities = if config.qualities.is_empty() {
        ascii_set(&default_accepted_qualities())
    } else {
        ascii_set(&config.qualities)
    };
    let filters = config.enabled.then(|| Filters {
        prefixes: exact_set(&config.prefixes),
        suffixes: exact_set(&config.suffixes),
        categories: exact_set(&config.categories),
        equipment_types: exact_set(&config.equipment_types),
        level_min: config.level_min,
        level_max: config.level_max,
    });
    (qualities, filters)
}

fn forum(config: &ForumSyncWorkerConfig) -> Result<Option<impl Serialize>, ConfigError> {
    #[derive(Serialize)]
    struct Forum<'a, R> {
        endpoint_origin: String,
        verify_post_body: bool,
        credential_world: &'a str,
        board_ids: (i32, i32),
        maximum_response_bytes: usize,
        maximum_topic_pages: u16,
        minimum_request_interval_millis: u64,
        request_timeout_seconds: u64,
        lease_duration_seconds: u64,
        equipment_colors: Vec<R>,
    }
    if !config.enabled {
        return Ok(None);
    }
    Ok(Some(Forum {
        endpoint_origin: config.endpoint_url()?.origin().ascii_serialization(),
        verify_post_body: config.verify_post_body,
        credential_world: &config.credential_world,
        board_ids: if config.test_board_mode {
            (4, 4)
        } else {
            (config.main_board_id, config.isekai_board_id)
        },
        maximum_response_bytes: config.maximum_response_bytes,
        maximum_topic_pages: config.maximum_topic_pages,
        minimum_request_interval_millis: config.minimum_request_interval_millis,
        request_timeout_seconds: config.request_timeout_seconds,
        lease_duration_seconds: config.lease_duration_seconds,
        equipment_colors: config.equipment_colors.iter().map(color_rule).collect(),
    }))
}

fn alerts(config: &OperationsAlertWorkerConfig) -> Option<impl Serialize> {
    #[derive(Serialize)]
    struct Alerts<'a> {
        notify_enabled: bool,
        webhook_template: Option<&'a str>,
        scan_interval_seconds: u64,
        jitter_seconds: u64,
        source_timeout_seconds: u64,
        batch_window_seconds: u64,
        cooldown_seconds: u64,
        claim_lease_seconds: u64,
        retry_delay_seconds: u64,
        max_signals_per_scan: usize,
        max_batch_items: usize,
        max_body_bytes: usize,
        delivery_pending_count_threshold: i64,
        delivery_pending_age_seconds: u64,
        delivery_manual_reconcile_count_threshold: i64,
        fulfillment_backlog_count_threshold: i64,
        fulfillment_backlog_age_seconds: u64,
        fulfillment_collection_overdue_age_seconds: u64,
        recovery_overdue_count_threshold: i64,
        recovery_overdue_age_seconds: u64,
        target: Option<(&'a str, &'a str)>,
    }
    config.scan_enabled().then(|| Alerts {
        notify_enabled: config.notify_enabled(),
        webhook_template: config.webhook_enabled().then_some(&config.webhook_template),
        scan_interval_seconds: config.scan_interval_seconds,
        jitter_seconds: config.jitter_seconds,
        source_timeout_seconds: config.source_timeout_seconds,
        batch_window_seconds: config.batch_window_seconds,
        cooldown_seconds: config.cooldown_seconds,
        claim_lease_seconds: config.claim_lease_seconds,
        retry_delay_seconds: config.retry_delay_seconds,
        max_signals_per_scan: config.max_signals_per_scan,
        max_batch_items: config.max_batch_items,
        max_body_bytes: config.max_body_bytes,
        delivery_pending_count_threshold: config.delivery_pending_count_threshold,
        delivery_pending_age_seconds: config.delivery_pending_age_seconds,
        delivery_manual_reconcile_count_threshold: config.delivery_manual_reconcile_count_threshold,
        fulfillment_backlog_count_threshold: config.fulfillment_backlog_count_threshold,
        fulfillment_backlog_age_seconds: config.fulfillment_backlog_age_seconds,
        fulfillment_collection_overdue_age_seconds: config
            .fulfillment_collection_overdue_age_seconds,
        recovery_overdue_count_threshold: config.recovery_overdue_count_threshold,
        recovery_overdue_age_seconds: config.recovery_overdue_age_seconds,
        target: config.notify_enabled().then_some((&config.world_id[..], &config.recipient[..])),
    })
}

fn loops(config: &WorkerConfig) -> impl Serialize {
    #[derive(Serialize)]
    struct Loops<'a> {
        concurrency: u16,
        poll_interval_seconds: u64,
        batch_limit: u16,
        jitter_seconds: u64,
        item_timeout_seconds: u64,
        recovery_retry_delay_seconds: u64,
        delivery_lease_duration_seconds: u64,
        delivery_renew_interval_seconds: u64,
        delivery_sending_grace_seconds: u64,
        fulfillment: &'a FulfillmentWorkerConfig,
        recovery: &'a RecoveryWorkerConfig,
    }
    Loops {
        concurrency: config.concurrency.unwrap_or(DEFAULT_WORKER_CONCURRENCY),
        poll_interval_seconds: config
            .poll_interval_seconds
            .unwrap_or(DEFAULT_WORKER_POLL_INTERVAL_SECONDS),
        batch_limit: config.batch_limit,
        jitter_seconds: config.jitter_seconds,
        item_timeout_seconds: config.item_timeout_seconds,
        recovery_retry_delay_seconds: config.recovery_retry_delay_seconds,
        delivery_lease_duration_seconds: config.delivery.lease_duration_seconds,
        delivery_renew_interval_seconds: config.delivery.renew_interval_seconds,
        delivery_sending_grace_seconds: config.delivery.sending_grace_seconds,
        fulfillment: &config.fulfillment,
        recovery: &config.recovery,
    }
}

fn color_rule(rule: &EquipmentColorRule) -> impl Serialize {
    #[derive(Serialize)]
    struct Rule<'a> {
        categories: Vec<String>,
        equipment_types: Vec<TypeSelection>,
        qualities: Vec<String>,
        level_min: i32,
        level_max: i32,
        prefixes: Vec<String>,
        suffixes: Vec<String>,
        prefix_color: &'a str,
        suffix_color: &'a str,
        combined_color: &'a str,
    }
    #[derive(Serialize)]
    struct TypeSelection {
        name: String,
        material_family: bool,
    }
    fn constraint(single: &str, multiple: &[String]) -> Vec<String> {
        if multiple.is_empty() && !single.is_empty() {
            vec![single.to_ascii_lowercase()]
        } else {
            ascii_set(multiple)
        }
    }
    // 旧单值按装备种类精确匹配；多选项里的材质名可以匹配全材质族。
    let equipment_types = constraint(&rule.equipment_type, &rule.equipment_types)
        .into_iter()
        .map(|name| TypeSelection {
            material_family: !rule.equipment_types.is_empty()
                && armor_material_category(&name).is_some(),
            name,
        })
        .collect();
    let mut suffixes = rule
        .suffixes
        .iter()
        .map(|value| {
            let mut value = value.to_ascii_lowercase();
            if value.starts_with("the ") { value.split_off(4) } else { value }
        })
        .collect::<Vec<_>>();
    suffixes.sort_unstable();
    suffixes.dedup();
    Rule {
        categories: constraint(&rule.category, &rule.categories),
        equipment_types,
        qualities: constraint(&rule.quality, &rule.qualities),
        level_min: rule.level_min,
        level_max: rule.level_max,
        prefixes: ascii_set(&rule.prefixes),
        suffixes,
        prefix_color: &rule.prefix_color,
        suffix_color: &rule.suffix_color,
        combined_color: &rule.combined_color,
    }
}

fn exact_set(values: &[String]) -> Vec<&str> {
    let mut sorted = values.iter().map(String::as_str).collect::<Vec<_>>();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

fn ascii_set(values: &[String]) -> Vec<String> {
    let mut sorted = values.iter().map(|value| value.to_ascii_lowercase()).collect::<Vec<_>>();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
}

#[cfg(test)]
mod tests {
    use super::digest;
    use crate::{AuctionConfig, MoogleMailWorldConfig, WorkerConfig};

    fn enabled_worker() -> WorkerConfig {
        let mut worker = WorkerConfig::default();
        worker.mooglemail.enabled = true;
        worker.forum.enabled = true;
        worker
    }

    #[test]
    fn cold_fields_change_the_digest_but_hot_fields_do_not() {
        let auction = AuctionConfig::default();
        let worker = enabled_worker();
        let before = digest(&auction, &worker, &[]).unwrap();
        let mut edited = worker.clone();
        edited.mooglemail.send_intervals.account_seconds += 1;
        edited.mooglemail.send_classes.account_reply =
            !edited.mooglemail.send_classes.account_reply;
        edited.mooglemail.batch_size += 1;
        edited.mooglemail.fetch_timeout_seconds += 1;
        edited.mooglemail.jitter_seconds += 1;
        edited.mooglemail.fetch_attempts += 1;
        edited.mooglemail.process_attempts += 1;
        edited.mooglemail.item_timeout_seconds += 1;
        edited.mooglemail.batch_timeout_seconds += 1;
        edited.mooglemail.retry_base_seconds += 1;
        edited.mooglemail.retry_maximum_seconds += 1;
        edited.mooglemail.account_reply_window_seconds += 1;
        edited.mooglemail.explanation_mail_window_seconds += 1;
        edited.mooglemail.explanation_mail_limit += 1;
        edited.forum.poll_interval_seconds += 1;
        edited.forum.item_timeout_seconds += 1;
        edited.delivery.batch_limit += 1;
        edited.delivery.request_timeout_seconds += 1;
        assert_eq!(digest(&auction, &edited, &[]).unwrap(), before);

        edited.delivery.sending_grace_seconds += 1;
        assert_ne!(digest(&auction, &edited, &[]).unwrap(), before);
        edited = worker.clone();
        edited.mooglemail.rekey_enabled = !edited.mooglemail.rekey_enabled;
        assert_ne!(digest(&auction, &edited, &[]).unwrap(), before);
        edited = worker;
        edited.forum.lease_duration_seconds += 1;
        assert_ne!(digest(&auction, &edited, &[]).unwrap(), before);
    }

    #[test]
    fn effective_defaults_and_unordered_filters_have_one_digest() {
        let mut auction = AuctionConfig::default();
        auction.schedule.enabled = true;
        auction.schedule.preview_time = "0:00".into();
        let mut worker = enabled_worker();
        worker.mooglemail.schedule = "CRON_TZ=UTC @hourly".into();
        worker.mooglemail.sender_allowlist = vec!["Alice".into(), "Bob".into()];
        let before = digest(&auction, &worker, &["Bot".into()]).unwrap();

        auction.schedule.preview_time = "00:00".into();
        auction.schedule.worlds = vec!["main".into()];
        worker.concurrency = Some(8);
        worker.poll_interval_seconds = Some(60);
        worker.mooglemail.schedule = "TZ=UTC 0 * * * *".into();
        worker.mooglemail.maintenance_schedule = Some("TZ=UTC @hourly".into());
        worker.mooglemail.start_price = Some(50_000);
        worker.mooglemail.sender_allowlist = vec!["bob".into(), "ALICE".into()];
        worker.mooglemail.equipment_filter.qualities.clear();
        worker.mooglemail.equipment_filter.prefixes = vec!["ignored".into()];
        assert_eq!(digest(&auction, &worker, &["bot".into()]).unwrap(), before);

        worker.mooglemail.sender_allowlist.pop();
        assert_ne!(digest(&auction, &worker, &["bot".into()]).unwrap(), before);
    }

    #[test]
    fn schedule_worlds_and_forum_priority_keep_their_order() {
        let mut auction = AuctionConfig::default();
        auction.schedule.enabled = true;
        let mut worker = enabled_worker();
        worker.mooglemail.additional_worlds.push(MoogleMailWorldConfig {
            world_id: "isekai".into(),
            mailbox_key: "mooglemail:isekai".into(),
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: true,
        });
        let before = digest(&auction, &worker, &[]).unwrap();
        auction.schedule.worlds.reverse();
        assert_ne!(digest(&auction, &worker, &[]).unwrap(), before);

        let doc: auction_application::admin::AdminRuntimeDocument = toml::from_str(
            r##"
            [[equipment_colors.rules]]
            prefixes = ["Ethereal"]
            prefix_color = "#00AA00"
            [[equipment_colors.rules]]
            prefixes = ["Ethereal"]
            prefix_color = "#00BB00"
            "##,
        )
        .unwrap();
        crate::settings::operations::apply_worker_runtime(&mut auction, &mut worker, doc);
        let before = digest(&auction, &worker, &[]).unwrap();
        worker.forum.equipment_colors.reverse();
        assert_ne!(digest(&auction, &worker, &[]).unwrap(), before);
    }

    #[test]
    fn forum_material_family_and_legacy_exact_match_have_different_digests() {
        let mut worker = enabled_worker();
        let mut auction = AuctionConfig::default();
        let doc: auction_application::admin::AdminRuntimeDocument = toml::from_str(
            r#"
            [[equipment_colors.rules]]
            equipment_type = "Cotton"
            prefixes = ["Ethereal"]
            "#,
        )
        .unwrap();
        crate::settings::operations::apply_worker_runtime(&mut auction, &mut worker, doc);
        let exact = digest(&auction, &worker, &[]).unwrap();

        let rule = &mut worker.forum.equipment_colors[0];
        rule.equipment_type = "cotton".into();
        assert_eq!(digest(&auction, &worker, &[]).unwrap(), exact);
        let rule = &mut worker.forum.equipment_colors[0];
        rule.equipment_types = vec![std::mem::take(&mut rule.equipment_type)];
        assert_ne!(digest(&auction, &worker, &[]).unwrap(), exact);
    }

    #[test]
    fn public_origin_changes_digest_even_without_forum_sync() {
        let auction = AuctionConfig::default();
        for enabled in [false, true] {
            let mut worker = WorkerConfig::default();
            worker.forum.enabled = enabled;
            let without = digest(&auction, &worker, &[]).unwrap();
            worker.forum.public_base_url = Some("https://example.invalid".into());
            let with = digest(&auction, &worker, &[]).unwrap();
            assert_ne!(with, without);
            worker.forum.public_base_url = Some("HTTPS://EXAMPLE.INVALID:443/".into());
            assert_eq!(digest(&auction, &worker, &[]).unwrap(), with);
            worker.forum.public_base_url = Some("https://other.invalid".into());
            assert_ne!(digest(&auction, &worker, &[]).unwrap(), with);
            worker.forum.public_base_url = Some("https://fixture:password@example.invalid".into());
            assert!(digest(&auction, &worker, &[]).is_err());
        }
    }

    #[test]
    fn webhook_template_changes_digest_only_when_webhook_is_enabled() {
        let auction = AuctionConfig::default();
        let mut worker = WorkerConfig::default();
        let disabled = digest(&auction, &worker, &[]).unwrap();
        worker.operations_alert.webhook_template = r#"{"format":"new"}"#.into();
        assert_eq!(digest(&auction, &worker, &[]).unwrap(), disabled);

        worker.operations_alert.webhook_enabled = true;
        let enabled = digest(&auction, &worker, &[]).unwrap();
        worker.operations_alert.webhook_template = r#"{"format":"other"}"#.into();
        assert_ne!(digest(&auction, &worker, &[]).unwrap(), enabled);
    }

    #[test]
    fn cold_endpoints_follow_enabled_channels_and_normalize_defaults() {
        let auction = AuctionConfig::default();
        let mut worker = WorkerConfig::default();
        let disabled = digest(&auction, &worker, &[]).unwrap();
        worker.mooglemail.endpoint = "http://127.0.0.1:8080".into();
        worker.forum.endpoint = Some("http://127.0.0.1:8081".into());
        assert_eq!(digest(&auction, &worker, &[]).unwrap(), disabled);

        worker.mooglemail.enabled = true;
        let mail_loopback = digest(&auction, &worker, &[]).unwrap();
        worker.mooglemail.endpoint = WorkerConfig::default().mooglemail.endpoint;
        let mail_default = digest(&auction, &worker, &[]).unwrap();
        assert_ne!(mail_loopback, mail_default);
        worker.mooglemail.endpoint = "HTTPS://HENTAIVERSE.ORG:443/".into();
        assert_eq!(digest(&auction, &worker, &[]).unwrap(), mail_default);

        worker.forum.enabled = true;
        let forum_loopback = digest(&auction, &worker, &[]).unwrap();
        worker.forum.endpoint = None;
        let forum_default = digest(&auction, &worker, &[]).unwrap();
        assert_ne!(forum_loopback, forum_default);
        worker.forum.endpoint = Some("HTTPS://FORUMS.E-HENTAI.ORG:443/".into());
        assert_eq!(digest(&auction, &worker, &[]).unwrap(), forum_default);
    }
}
