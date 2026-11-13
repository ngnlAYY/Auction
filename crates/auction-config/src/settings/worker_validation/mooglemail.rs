//! MoogleMail Worker 配置的边界校验。

use std::collections::HashSet;

use super::super::{
    BTreeSet, ConfigError, MoogleMailEquipmentFilterConfig, MoogleMailWorkerConfig,
    RecurringSchedule, StandardCronSchedule, Url, Utc, valid_config_text,
};

pub(crate) fn validate_mooglemail_worker(
    config: &MoogleMailWorkerConfig,
) -> Result<(), ConfigError> {
    validate_mooglemail_world(&config.world_id, &config.mailbox_key, &config.sender_allowlist)?;
    if !config.send_intervals.is_valid() {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "send_intervals 各业务组必须位于 0 至 86400 秒",
        ));
    }
    if config.diagnostics.retention_days == 0 || config.diagnostics.retention_days > 365 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "diagnostics.retention_days 必须位于 1 至 365",
        ));
    }
    if config.diagnostics.maximum_total_bytes < 16 * 1024 * 1024
        || config.diagnostics.maximum_total_bytes > 1024 * 1024 * 1024
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "diagnostics.maximum_total_bytes 必须位于 16 MiB 至 1 GiB",
        ));
    }
    if config.diagnostics.directory.as_os_str().is_empty()
        || config.diagnostics.directory.to_string_lossy().chars().any(char::is_control)
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "diagnostics.directory 必须是非空且不含控制字符的路径",
        ));
    }
    if !(60..=604_800).contains(&config.account_reply_window_seconds) {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "account_reply_window_seconds 必须位于 60 至 604800",
        ));
    }
    if !(60..=604_800).contains(&config.explanation_mail_window_seconds) {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "explanation_mail_window_seconds 必须位于 60 至 604800",
        ));
    }
    if config.explanation_mail_limit > 100 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "explanation_mail_limit 必须位于 0 至 100",
        ));
    }
    if config.start_price.is_some_and(|value| value <= 0) {
        return Err(ConfigError::InvalidMoogleMailConfig("start_price 覆盖值必须大于零"));
    }
    if config.batch_size == 0 || config.batch_size > 1000 {
        return Err(ConfigError::InvalidMoogleMailConfig("batch_size 必须位于 1 至 1000"));
    }
    if config.maximum_pages == 0 || config.maximum_pages > 100 {
        return Err(ConfigError::InvalidMoogleMailConfig("maximum_pages 必须位于 1 至 100"));
    }
    if !valid_mooglemail_endpoint(&config.endpoint) {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "endpoint 必须是固定生产 HTTPS origin 或无附加字段的回环 HTTP(S) origin",
        ));
    }
    if config.maximum_response_bytes == 0 || config.maximum_response_bytes > 8 * 1024 * 1024 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "maximum_response_bytes 必须位于 1 至 8 MiB",
        ));
    }
    if config.minimum_request_interval_millis == 0
        || config.minimum_request_interval_millis > 60_000
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "minimum_request_interval_millis 必须位于 1 至 60000",
        ));
    }
    if !(1..=300).contains(&config.request_timeout_seconds) {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "request_timeout_seconds 必须位于 1 至 300",
        ));
    }
    let schedule = StandardCronSchedule::parse(&config.schedule).map_err(|_| {
        ConfigError::InvalidMoogleMailConfig(
            "schedule 必须是标准五字段 cron、受支持描述符或 @every duration",
        )
    })?;
    if schedule.next_after(Utc::now()).is_none() {
        return Err(ConfigError::InvalidMoogleMailConfig("schedule 必须存在可计算的未来触发时刻"));
    }
    if let Some(expression) = &config.maintenance_schedule {
        let schedule = StandardCronSchedule::parse(expression).map_err(|_| {
            ConfigError::InvalidMoogleMailConfig(
                "maintenance_schedule 必须是标准五字段 cron、受支持描述符或 @every duration",
            )
        })?;
        if schedule.next_after(Utc::now()).is_none() {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "maintenance_schedule 必须存在可计算的未来触发时刻",
            ));
        }
    }
    if !(60..=86_400).contains(&config.read_mail_observation_interval_seconds) {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "read_mail_observation_interval_seconds 必须位于 60 至 86400",
        ));
    }
    if !(60..=604_800).contains(&config.mail_scan_daily_full_interval_seconds) {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "mail_scan_daily_full_interval_seconds 必须位于 60 至 604800",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidMoogleMailConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "lease_duration_seconds 必须位于 1 至 86400",
        ));
    }
    if config.renew_interval_seconds == 0
        || config.renew_interval_seconds >= config.lease_duration_seconds
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "renew_interval_seconds 必须大于 0 且小于 lease_duration_seconds",
        ));
    }
    if !(1..=10).contains(&config.fetch_attempts) {
        return Err(ConfigError::InvalidMoogleMailConfig("fetch_attempts 必须位于 1 至 10"));
    }
    if !(1..=300).contains(&config.fetch_timeout_seconds)
        || config.fetch_timeout_seconds <= config.request_timeout_seconds
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "fetch_timeout_seconds 必须位于 1 至 300 且大于 request_timeout_seconds",
        ));
    }
    if !(1..=10).contains(&config.process_attempts) {
        return Err(ConfigError::InvalidMoogleMailConfig("process_attempts 必须位于 1 至 10"));
    }
    if !(1..=300).contains(&config.item_timeout_seconds)
        || config.item_timeout_seconds <= config.request_timeout_seconds
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "item_timeout_seconds 必须位于 1 至 300 且大于 request_timeout_seconds",
        ));
    }
    if !(1..=86_400).contains(&config.batch_timeout_seconds)
        || config.batch_timeout_seconds
            < config.fetch_timeout_seconds.saturating_add(config.item_timeout_seconds)
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "batch_timeout_seconds 必须位于 1 至 86400 且至少覆盖一次 fetch_timeout_seconds 与 item_timeout_seconds",
        ));
    }
    if config.retry_base_seconds == 0
        || config.retry_base_seconds > config.retry_maximum_seconds
        || config.retry_maximum_seconds > 300
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "retry_base_seconds 与 retry_maximum_seconds 必须位于 1 至 300 且 maximum 不小于 base",
        ));
    }
    validate_mooglemail_equipment_filter(&config.equipment_filter)?;
    config
        .mail_templates
        .validate()
        .map_err(|(_key, error)| ConfigError::InvalidMoogleMailConfig(error.message()))?;
    let mut worlds = BTreeSet::new();
    worlds.insert(config.world_id.as_str());
    for world in &config.additional_worlds {
        validate_mooglemail_world(&world.world_id, &world.mailbox_key, &world.sender_allowlist)?;
        if !worlds.insert(world.world_id.as_str()) {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "additional_worlds 不得重复配置同一 world_id",
            ));
        }
    }
    let mut mailboxes = BTreeSet::new();
    mailboxes.insert(config.mailbox_key.as_str());
    for world in &config.additional_worlds {
        if !mailboxes.insert(world.mailbox_key.as_str()) {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "additional_worlds 的 mailbox_key 必须唯一",
            ));
        }
    }
    Ok(())
}

pub(crate) fn valid_mooglemail_endpoint(value: &str) -> bool {
    if !auction_domain::valid_text(value, 2048) {
        return false;
    }
    let Ok(endpoint) = Url::parse(value) else {
        return false;
    };
    let path_is_root = endpoint.path().is_empty() || endpoint.path() == "/";
    let common = endpoint.host_str().is_some()
        && endpoint.username().is_empty()
        && endpoint.password().is_none()
        && path_is_root
        && endpoint.query().is_none()
        && endpoint.fragment().is_none();
    if !common {
        return false;
    }
    if endpoint.scheme() == "https"
        && endpoint.host_str().is_some_and(|host| host.eq_ignore_ascii_case("hentaiverse.org"))
        && endpoint.port().is_none()
    {
        return true;
    }
    if !matches!(endpoint.scheme(), "http" | "https") {
        return false;
    }
    match endpoint.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

pub(crate) fn validate_mooglemail_equipment_filter(
    config: &MoogleMailEquipmentFilterConfig,
) -> Result<(), ConfigError> {
    if !(0..=1000).contains(&config.level_min)
        || !(0..=1000).contains(&config.level_max)
        || config.level_max != 0 && config.level_max < config.level_min
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "equipment_filter 的等级边界必须位于 0 至 1000；0 表示不限制，非零 max 不能小于 min",
        ));
    }
    let lists = [
        &config.qualities,
        &config.prefixes,
        &config.suffixes,
        &config.categories,
        &config.equipment_types,
    ];
    for list in lists {
        if list.len() > 256 {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "equipment_filter 每个 token 列表最多 256 项",
            ));
        }
        let mut seen = HashSet::with_capacity(list.len());
        for value in list {
            if !valid_config_text(value, 128) || !seen.insert(value.as_str()) {
                return Err(ConfigError::InvalidMoogleMailConfig(
                    "equipment_filter token 必须是唯一、非空且不含控制字符的文本",
                ));
            }
        }
    }
    // 品质门槛始终存在（空列表也回退默认 L/P），因此不再要求其它过滤维度。
    Ok(())
}

pub(crate) fn validate_mooglemail_world(
    world_id: &str,
    mailbox_key: &str,
    sender_allowlist: &[String],
) -> Result<(), ConfigError> {
    if !valid_config_text(mailbox_key, 256) {
        return Err(ConfigError::InvalidMoogleMailConfig("mailbox_key 无效"));
    }
    if !matches!(world_id, "main" | "isekai") {
        return Err(ConfigError::InvalidMoogleMailConfig("world_id 只能是 main 或 isekai"));
    }
    if sender_allowlist.len() > 1000 {
        return Err(ConfigError::InvalidMoogleMailConfig("sender_allowlist 最多 1000 项"));
    }
    // 借用原文建索引，避免千项名单逐项扫描前缀；不归一化、重排或复制发件人。
    let mut seen = HashSet::with_capacity(sender_allowlist.len());
    for sender in sender_allowlist {
        if !valid_config_text(sender, 128) || !seen.insert(sender.as_str()) {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "sender_allowlist 含有重复或无效发件人",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::settings::worker_validation::mooglemail as parent;

    #[test]
    fn mail_scan_daily_full_interval_is_bounded_and_defaults_to_one_day() {
        let default = MoogleMailWorkerConfig::default();
        assert_eq!(default.mail_scan_daily_full_interval_seconds, 86_400);
        assert!(validate_mooglemail_worker(&default).is_ok());

        for accepted in [60, 3_600, 86_400, 604_800] {
            let config = MoogleMailWorkerConfig {
                mail_scan_daily_full_interval_seconds: accepted,
                ..MoogleMailWorkerConfig::default()
            };
            assert!(validate_mooglemail_worker(&config).is_ok(), "{accepted} 应被接受");
        }
        for rejected in [0, 59, 604_801] {
            let config = MoogleMailWorkerConfig {
                mail_scan_daily_full_interval_seconds: rejected,
                ..MoogleMailWorkerConfig::default()
            };
            let error = validate_mooglemail_worker(&config).expect_err("越界间隔必须被拒绝");
            assert!(
                matches!(error, ConfigError::InvalidMoogleMailConfig(message) if message.contains("mail_scan_daily_full_interval_seconds"))
            );
        }
    }
}
