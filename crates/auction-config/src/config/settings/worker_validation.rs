//! Worker、论坛、MoogleMail 和运维告警配置校验。

use super::*;

pub(super) fn validate_auction_worker(
    config: &WorkerConfig,
    concurrency: u16,
    poll_interval_seconds: u64,
) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidAuctionWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if concurrency == 0 || concurrency > config.batch_limit {
        return Err(ConfigError::InvalidAuctionWorkerConfig(
            "concurrency 必须位于 1 至 batch_limit",
        ));
    }
    if poll_interval_seconds == 0 || poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidAuctionWorkerConfig(
            "poll_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidAuctionWorkerConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    let lease_duration_seconds = poll_interval_seconds.saturating_mul(3).max(30);
    if config.item_timeout_seconds == 0
        || config.item_timeout_seconds >= lease_duration_seconds
        || config.item_timeout_seconds > 86_400
    {
        return Err(ConfigError::InvalidAuctionWorkerConfig(
            "item_timeout_seconds 必须大于 0、小于派生 claim lease 且不超过 86400",
        ));
    }
    Ok(())
}

pub(super) fn validate_delivery_worker(config: &DeliveryWorkerConfig) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if config.concurrency == 0 || config.concurrency > config.batch_limit {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "concurrency 必须位于 1 至 batch_limit",
        ));
    }
    if config.poll_interval_seconds == 0 || config.poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "poll_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "lease_duration_seconds 必须位于 1 至 86400",
        ));
    }
    if config.renew_interval_seconds == 0
        || config.renew_interval_seconds >= config.lease_duration_seconds
    {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "renew_interval_seconds 必须大于 0 且小于 lease_duration_seconds",
        ));
    }
    if config.request_timeout_seconds == 0 || config.request_timeout_seconds > 86_400 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "request_timeout_seconds 必须位于 1 至 86400",
        ));
    }
    if config.sending_grace_seconds <= config.request_timeout_seconds
        || config.sending_grace_seconds > 86_400
    {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "sending_grace_seconds 必须大于 request_timeout_seconds 且不超过 86400",
        ));
    }
    if config.finalize_timeout_seconds == 0 || config.finalize_timeout_seconds > 300 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "finalize_timeout_seconds 必须位于 1 至 300",
        ));
    }
    Ok(())
}

pub(super) fn validate_fulfillment_worker(
    config: &FulfillmentWorkerConfig,
) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if config.concurrency == 0 || config.concurrency > config.batch_limit {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "concurrency 必须位于 1 至 batch_limit",
        ));
    }
    if config.poll_interval_seconds == 0 || config.poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "poll_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "jitter_seconds 必须位于 0 至 86400",
        ));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "lease_duration_seconds 必须位于 1 至 86400",
        ));
    }
    if config.item_timeout_seconds == 0
        || config.item_timeout_seconds >= config.lease_duration_seconds
    {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "item_timeout_seconds 必须大于 0 且小于 lease_duration_seconds",
        ));
    }
    if config.retry_delay_seconds == 0 || config.retry_delay_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "retry_delay_seconds 必须位于 1 至 86400",
        ));
    }
    Ok(())
}

pub(super) fn validate_recovery_worker(config: &RecoveryWorkerConfig) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if config.concurrency == 0 || config.concurrency > config.batch_limit {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "concurrency 必须位于 1 至 batch_limit",
        ));
    }
    if config.poll_interval_seconds == 0 || config.poll_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "poll_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > 86_400 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig("jitter_seconds 必须位于 0 至 86400"));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "lease_duration_seconds 必须位于 1 至 86400",
        ));
    }
    if config.item_timeout_seconds == 0
        || config.item_timeout_seconds >= config.lease_duration_seconds
    {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "item_timeout_seconds 必须大于 0 且小于 lease_duration_seconds",
        ));
    }
    if config.low_balance_threshold < 0 {
        return Err(ConfigError::InvalidRecoveryWorkerConfig("low_balance_threshold 不能为负值"));
    }
    if config.health_refresh_interval_seconds == 0
        || config.health_refresh_interval_seconds > 86_400
    {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "health_refresh_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.evidence_stale_after_seconds < config.health_refresh_interval_seconds
        || config.evidence_stale_after_seconds > 86_400
    {
        return Err(ConfigError::InvalidRecoveryWorkerConfig(
            "evidence_stale_after_seconds 必须不小于 health_refresh_interval_seconds 且不超过 86400",
        ));
    }
    Ok(())
}

pub(super) fn validate_worker_control(config: &WorkerControlConfig) -> Result<(), ConfigError> {
    let socket_path = Path::new(&config.socket_path);
    let runtime_directory = Path::new(&config.runtime_directory);
    if !valid_worker_control_path(runtime_directory, None) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "runtime_directory 必须是规范化的非根绝对路径",
        ));
    }
    if !valid_worker_control_path(socket_path, Some(100)) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "socket_path 必须是规范化绝对路径且长度不超过 100 字节",
        ));
    }
    if socket_path.parent() != Some(runtime_directory) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "socket_path 的直接父目录必须等于 runtime_directory",
        ));
    }
    if !valid_safe_identifier(&config.group) {
        return Err(ConfigError::InvalidWorkerControlConfig("group 必须是 1 至 64 位安全标识"));
    }
    if config.allowed_peer_uids.len() + config.allowed_peer_gids.len() > 64 {
        return Err(ConfigError::InvalidWorkerControlConfig("peer allowlist 不能超过 64 项"));
    }
    if duplicate_u32(&config.allowed_peer_uids) || duplicate_u32(&config.allowed_peer_gids) {
        return Err(ConfigError::InvalidWorkerControlConfig("peer allowlist 不能包含重复 ID"));
    }
    if config.enabled && config.allowed_peer_uids.is_empty() && config.allowed_peer_gids.is_empty()
    {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "启用 Worker control 时必须显式配置 peer UID 或 GID allowlist",
        ));
    }
    Ok(())
}

pub(super) fn valid_worker_control_path(path: &Path, maximum_bytes: Option<usize>) -> bool {
    let normalized: PathBuf = path.components().collect();
    path.is_absolute()
        && path != Path::new("/")
        && normalized.as_os_str() == path.as_os_str()
        && !path.components().any(|component| {
            matches!(component, std::path::Component::CurDir | std::path::Component::ParentDir)
        })
        && !path.as_os_str().to_string_lossy().chars().any(|character| character == '\0')
        && maximum_bytes.is_none_or(|maximum| path.as_os_str().len() <= maximum)
}

pub(super) fn valid_safe_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(super) fn duplicate_u32(values: &[u32]) -> bool {
    let mut values = values.to_vec();
    values.sort_unstable();
    values.windows(2).any(|pair| pair[0] == pair[1])
}

pub(super) fn validate_mooglemail_worker(
    config: &MoogleMailWorkerConfig,
) -> Result<(), ConfigError> {
    validate_mooglemail_world(&config.world_id, &config.mailbox_key, &config.sender_allowlist)?;
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

pub(super) fn valid_mooglemail_endpoint(value: &str) -> bool {
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

pub(super) fn validate_mooglemail_equipment_filter(
    config: &MoogleMailEquipmentFilterConfig,
) -> Result<(), ConfigError> {
    if config.level_min < 0 || config.level_max < config.level_min || config.level_max > 1000 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "equipment_filter 的等级范围必须位于 0 至 1000 且 max 不小于 min",
        ));
    }
    let lists = [
        &config.qualities,
        &config.prefixes,
        &config.suffixes,
        &config.categories,
        &config.equipment_types,
    ];
    let mut filter_count = 0;
    for list in lists {
        if list.len() > 256 {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "equipment_filter 每个 token 列表最多 256 项",
            ));
        }
        filter_count += list.len();
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
    if config.enabled && filter_count == 0 && config.level_min == 0 && config.level_max == 0 {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "启用 equipment_filter 时至少需要一个过滤条件",
        ));
    }
    Ok(())
}

pub(super) fn validate_mooglemail_notification_templates(
    config: &MoogleMailNotificationTemplatesConfig,
) -> Result<(), ConfigError> {
    let templates = [
        ("auction_assets_rejected", config.auction_assets_rejected.as_str(), &[][..], &[][..]),
        ("auction_no_attachments", config.auction_no_attachments.as_str(), &[][..], &[][..]),
        (
            "auction_equipment_rejected",
            config.auction_equipment_rejected.as_str(),
            &["reason_code"][..],
            &["reason_code"][..],
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
            &["result_code", "username"][..],
            &["result_code", "username"][..],
        ),
        (
            "rebind_result",
            config.rebind_result.as_str(),
            &["result_code", "username"][..],
            &["result_code", "username"][..],
        ),
        (
            "settlement_winner_equipment_cod",
            config.settlement_winner_equipment_cod.as_str(),
            &["item_name", "amount", "reference"][..],
            &["item_name", "amount", "reference"][..],
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
    Ok(())
}

pub(super) fn validate_mooglemail_template_body(
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

pub(super) fn validate_mooglemail_world(
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
    for (index, sender) in sender_allowlist.iter().enumerate() {
        if !valid_config_text(sender, 256)
            || sender_allowlist[..index]
                .iter()
                .any(|previous| previous.eq_ignore_ascii_case(sender))
        {
            return Err(ConfigError::InvalidMoogleMailConfig(
                "sender_allowlist 含有重复或无效发件人",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_forum_worker(config: &ForumSyncWorkerConfig) -> Result<(), ConfigError> {
    if !matches!(config.credential_world.as_str(), "main" | "isekai" | "shared") {
        return Err(ConfigError::InvalidForumConfig(
            "credential_world 只能是 main、isekai 或 shared",
        ));
    }
    if config.batch_size == 0 || config.batch_size > 100 {
        return Err(ConfigError::InvalidForumConfig("batch_size 必须位于 1 至 100"));
    }
    if config.maximum_response_bytes == 0 || config.maximum_response_bytes > 8 * 1024 * 1024 {
        return Err(ConfigError::InvalidForumConfig("maximum_response_bytes 必须位于 1 至 8 MiB"));
    }
    if config.minimum_request_interval_millis == 0 {
        return Err(ConfigError::InvalidForumConfig("minimum_request_interval_millis 必须大于零"));
    }
    if !(1..=300).contains(&config.request_timeout_seconds) {
        return Err(ConfigError::InvalidForumConfig("request_timeout_seconds 必须位于 1 至 300"));
    }
    if config.item_timeout_seconds == 0 || config.item_timeout_seconds > 3_600 {
        return Err(ConfigError::InvalidForumConfig("item_timeout_seconds 必须位于 1 至 3600"));
    }
    let request_budget_millis = config
        .request_timeout_seconds
        .saturating_mul(3)
        .saturating_mul(1_000)
        .saturating_add(config.minimum_request_interval_millis.saturating_mul(2))
        .saturating_add(1_000);
    if config.item_timeout_seconds.saturating_mul(1_000) < request_budget_millis {
        return Err(ConfigError::InvalidForumConfig(
            "item_timeout_seconds 必须覆盖三次请求、两次最小请求间隔和一秒收口预算",
        ));
    }
    if config.lease_duration_seconds == 0 || config.lease_duration_seconds > 86_400 {
        return Err(ConfigError::InvalidForumConfig("lease_duration_seconds 必须位于 1 至 86400"));
    }
    if config.item_timeout_seconds >= config.lease_duration_seconds {
        return Err(ConfigError::InvalidForumConfig(
            "item_timeout_seconds 必须小于 lease_duration_seconds",
        ));
    }
    if let Some(endpoint) = &config.endpoint {
        let parsed = Url::parse(endpoint)
            .map_err(|_| ConfigError::InvalidForumConfig("endpoint 不是有效 URL"))?;
        let loopback_http = parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if !(parsed.scheme() == "https" || loopback_http)
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(ConfigError::InvalidForumConfig(
                "endpoint 必须是无凭据的 HTTPS origin 或回环 HTTP origin",
            ));
        }
    }
    if let Some(public_base_url) = &config.public_base_url {
        let parsed = Url::parse(public_base_url)
            .map_err(|_| ConfigError::InvalidForumConfig("public_base_url 不是有效 URL"))?;
        if !(parsed.scheme() == "https"
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"))))
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.path() != ""
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || public_base_url.trim() != public_base_url
        {
            return Err(ConfigError::InvalidForumConfig(
                "public_base_url 必须是无凭据、无路径和无查询的 HTTPS origin 或回环 HTTP origin",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_operations_alert_worker(
    config: &OperationsAlertWorkerConfig,
) -> Result<(), ConfigError> {
    if config.scan_interval_seconds == 0 || config.scan_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "scan_interval_seconds 必须位于 1 至 86400",
        ));
    }
    let policy = config.policy();
    if policy.validate().is_err() {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "时间、阈值、批次数量或正文大小超出允许范围",
        ));
    }
    if !matches!(config.world_id.as_str(), "main" | "isekai") {
        return Err(ConfigError::InvalidOperationsAlertConfig("world_id 只能是 main 或 isekai"));
    }
    if config.enabled && !valid_config_text(&config.recipient, 256) {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警时 recipient 必须是非空且不含控制字符的文本",
        ));
    }
    if !config.recipient.is_empty() && !valid_config_text(&config.recipient, 256) {
        return Err(ConfigError::InvalidOperationsAlertConfig("recipient 无效"));
    }
    Ok(())
}

pub(super) fn valid_config_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}
