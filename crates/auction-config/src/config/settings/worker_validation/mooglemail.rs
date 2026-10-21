use super::*;

pub(crate) fn validate_mooglemail_worker(
    config: &MoogleMailWorkerConfig,
) -> Result<(), ConfigError> {
    validate_mooglemail_world(&config.world_id, &config.mailbox_key, &config.sender_allowlist)?;
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
    validate_mooglemail_notification_templates(&config.mail_templates)?;
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
    if !mailboxes.insert(config.mailbox_key.as_str()) {
        return Err(ConfigError::InvalidMoogleMailConfig("mailbox_key 必须唯一"));
    }
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
    if value.is_empty()
        || value != value.trim()
        || value.len() > 2048
        || value.chars().any(char::is_control)
    {
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
    let Some(host) = endpoint.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|address| address.is_loopback())
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
        for (index, value) in list.iter().enumerate() {
            if !valid_config_text(value, 128)
                || list[..index].iter().any(|previous| previous == value)
            {
                return Err(ConfigError::InvalidMoogleMailConfig(
                    "equipment_filter token 必须是唯一、非空且不含控制字符的文本",
                ));
            }
        }
    }
    // 品质门槛始终存在（空列表也回退默认 L/P），因此不再要求其它过滤维度。
    Ok(())
}

pub(crate) fn validate_mooglemail_notification_templates(
    config: &MoogleMailNotificationTemplatesConfig,
) -> Result<(), ConfigError> {
    let templates = [
        ("auction_assets_rejected", config.auction_assets_rejected.as_str(), &[][..], &[][..]),
        ("auction_no_attachments", config.auction_no_attachments.as_str(), &[][..], &[][..]),
        (
            "auction_equipment_rejected",
            config.auction_equipment_rejected.as_str(),
            &[][..],
            &["reason_code", "reason_text", "next_steps", "mail_reference"][..],
        ),
        ("user_assets_rejected", config.user_assets_rejected.as_str(), &[][..], &[][..]),
        (
            "register_key",
            config.register_key.as_str(),
            &["login_key", "username"][..],
            &["login_key", "username"][..],
        ),
        (
            "register_result",
            config.register_result.as_str(),
            &["result_code"][..],
            &["result_code", "username"][..],
        ),
        (
            "rebind_result",
            config.rebind_result.as_str(),
            &["result_code"][..],
            &["result_code", "username"][..],
        ),
        (
            "settlement_winner_equipment_cod",
            config.settlement_winner_equipment_cod.as_str(),
            &["item_name", "amount", "reference"][..],
            &["item_name", "amount", "reference", "price_breakdown"][..],
        ),
        (
            "settlement_payment_carrier",
            config.settlement_payment_carrier.as_str(),
            &["amount", "reference"][..],
            &["amount", "reference"][..],
        ),
        (
            "settlement_payment_reminder",
            config.settlement_payment_reminder.as_str(),
            &["reference"][..],
            &["reference"][..],
        ),
        (
            "settlement_equipment_handoff",
            config.settlement_equipment_handoff.as_str(),
            &["item_name", "reference"][..],
            &["item_name", "reference"][..],
        ),
        (
            "settlement_seller_payout",
            config.settlement_seller_payout.as_str(),
            &["amount", "reference"][..],
            &["amount", "reference"][..],
        ),
        (
            "settlement_buyer_refund",
            config.settlement_buyer_refund.as_str(),
            &["amount", "reference"][..],
            &["amount", "reference"][..],
        ),
        ("operations_alert", config.operations_alert.as_str(), &["summary"][..], &["summary"][..]),
    ];
    for (_name, value, required, allowed) in templates {
        validate_mooglemail_template_body(value, required, allowed)?;
    }
    // 保留旧自定义正文的原因码，也允许新正文用可读原因替代；至少展示其中一种原因。
    if !config.auction_equipment_rejected.contains("{reason_code}")
        && !config.auction_equipment_rejected.contains("{reason_text}")
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "mail_templates.auction_equipment_rejected 必须包含 reason_code 或 reason_text",
        ));
    }
    Ok(())
}

pub(crate) fn validate_mooglemail_template_body(
    value: &str,
    required: &[&str],
    allowed: &[&str],
) -> Result<(), ConfigError> {
    if value.is_empty()
        || value != value.trim()
        || value.len() > 64 * 1024
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ConfigError::InvalidMoogleMailConfig("mail_templates 正文格式无效"));
    }
    let mut cursor = 0;
    let mut seen = BTreeSet::new();
    while let Some(relative_start) = value[cursor..].find('{') {
        let start = cursor + relative_start;
        let relative_end = value[start + 1..]
            .find('}')
            .ok_or(ConfigError::InvalidMoogleMailConfig("mail_templates 占位符未闭合"))?;
        let end = start + 1 + relative_end;
        let placeholder = &value[start + 1..end];
        if !allowed.contains(&placeholder) {
            return Err(ConfigError::InvalidMoogleMailConfig("mail_templates 含有未知占位符"));
        }
        seen.insert(placeholder);
        cursor = end + 1;
    }
    if value[cursor..].contains('}')
        || required.iter().any(|placeholder| !seen.contains(placeholder))
    {
        return Err(ConfigError::InvalidMoogleMailConfig("mail_templates 缺少必需占位符"));
    }
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
    for (index, sender) in sender_allowlist.iter().enumerate() {
        if !valid_config_text(sender, 128)
            || sender_allowlist[..index].iter().any(|previous| previous == sender)
        {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "sender_allowlist 含有重复或无效发件人",
            ));
        }
    }
    Ok(())
}
