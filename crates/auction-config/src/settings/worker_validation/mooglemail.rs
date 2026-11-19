//! MoogleMail Worker 配置的边界校验。

use std::collections::{BTreeSet, HashSet};

use auction_application::mail::{EquipmentFilterBoundsError, validate_equipment_filter_bounds};
use auction_application::scheduling::RecurringSchedule;
use auction_domain::{DataKeyDomain, valid_key_id, valid_text};
use auction_time::Timestamp;
use url::Url;

use super::super::{ConfigError, MoogleMailEquipmentFilterConfig, MoogleMailWorkerConfig};
use crate::StandardCronSchedule;
use crate::settings::secrets::{AppSecrets, SecretString, load_mooglemail_keyenv};

/// 主动告警依赖完整的邮件 transport；配置校验只检查本地凭据来源，不探测外部账号。
pub(super) fn validate_notification_mail_credentials(
    config: &MoogleMailWorkerConfig,
    secrets: &AppSecrets,
) -> Result<(), ConfigError> {
    if secrets.equipment_data_key.is_none() {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "启用告警主动通知时 MoogleMail 需要 equipment_data_key",
        ));
    }
    if secrets.user_login_data_key.is_none() {
        return Err(ConfigError::MissingMoogleMailUserLoginDataKey);
    }
    let keyrings = [
        (
            DataKeyDomain::Equipment,
            &secrets.equipment_data_key_id,
            &secrets.equipment_data_key,
            &secrets.equipment_previous_data_keys,
        ),
        (
            DataKeyDomain::UserLogin,
            &secrets.user_login_data_key_id,
            &secrets.user_login_data_key,
            &secrets.user_login_previous_data_keys,
        ),
        (
            DataKeyDomain::Credential,
            &secrets.credential_data_key_id,
            &secrets.credential_data_key,
            &secrets.credential_previous_data_keys,
        ),
        (
            DataKeyDomain::MailDiagnostics,
            &secrets.mail_diagnostics_data_key_id,
            &secrets.mail_diagnostics_data_key,
            &secrets.mail_diagnostics_previous_data_keys,
        ),
    ];
    for (domain, id, material, previous) in keyrings {
        if domain == DataKeyDomain::MailDiagnostics && !config.diagnostics.enabled {
            continue;
        }
        if id.as_ref().is_some_and(|id| !valid_key_id(id.expose_secret()))
            || material.as_ref().is_some_and(|key| !valid_data_key_material(key.expose_secret()))
            || previous.iter().any(|key| !valid_data_key_material(key.material()))
        {
            return Err(ConfigError::InvalidDataKeyRing {
                domain: domain.as_str(),
                reason: "MoogleMail 需要合法标识和标准 Base64 编码的 32 字节密钥".to_owned(),
            });
        }
    }
    // 凭据密钥环优先于 keyenv 或静态 Cookie，数据库记录由运行时检查。
    if secrets.credential_data_key.is_some() {
        return Ok(());
    }
    if let Some(path) = &config.keyenv_file {
        load_mooglemail_keyenv(path)?;
    } else {
        let cookie = secrets
            .mooglemail_cookie
            .as_ref()
            .map(SecretString::expose_secret)
            .ok_or(ConfigError::MissingMoogleMailCredentials)?;
        if !valid_text(cookie.trim(), 4096) {
            return Err(ConfigError::InvalidMoogleMailConfig("mooglemail_cookie 格式无效"));
        }
    }
    Ok(())
}

fn valid_data_key_material(value: &str) -> bool {
    let [prefix @ .., last, b'='] = value.as_bytes() else {
        return false;
    };
    // 32 字节的标准 Base64 末尾有一个补位符，末个有效字符的低两位必须为零。
    prefix.len() == 42
        && prefix.iter().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
        && b"AEIMQUYcgkosw048".contains(last)
}

pub(crate) fn validate_mooglemail_worker(
    config: &MoogleMailWorkerConfig,
) -> Result<(), ConfigError> {
    validate_mooglemail_world(
        &config.world_id,
        &config.mailbox_key,
        &config.sender_allowlist,
        config.enabled,
    )?;
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
    if config.diagnostics.directory.is_empty()
        || config.diagnostics.directory.to_string_lossy().chars().any(char::is_control)
        || config.enabled
            && config.diagnostics.enabled
            && config.diagnostics.directory.file_name().is_none()
    {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "diagnostics.directory 必须是非空且不含控制字符的路径，启用时必须指向具名目录",
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
    config.endpoint_url()?;
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
    let schedule = config.receive_schedule()?;
    if schedule.next_after(Timestamp::now()).is_none() {
        return Err(ConfigError::InvalidMoogleMailConfig("schedule 必须存在可计算的未来触发时刻"));
    }
    if let Some(expression) = &config.maintenance_schedule {
        let schedule = StandardCronSchedule::parse(expression).map_err(|_| {
            ConfigError::InvalidMoogleMailConfig(
                "maintenance_schedule 必须是标准五字段 cron、受支持描述符或 @every duration",
            )
        })?;
        if schedule.next_after(Timestamp::now()).is_none() {
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
    if config.removed_mail_templates {
        return Err(ConfigError::InvalidMoogleMailConfig(
            "mail_templates 已迁入数据库，由 Admin 的“邮件模板”维护；请从 Worker 配置删除 [worker.mooglemail.mail_templates] 区块",
        ));
    }
    let mut worlds = BTreeSet::new();
    worlds.insert(config.world_id.as_str());
    for world in &config.additional_worlds {
        validate_mooglemail_world(
            &world.world_id,
            &world.mailbox_key,
            &world.sender_allowlist,
            config.enabled,
        )?;
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

impl MoogleMailWorkerConfig {
    /// 用 Worker 启动校验的边界解析邮件 origin，供冷配置投影使用。
    pub(crate) fn endpoint_url(&self) -> Result<Url, ConfigError> {
        parse_mooglemail_endpoint(&self.endpoint).ok_or(ConfigError::InvalidMoogleMailConfig(
            "endpoint 必须是固定生产 HTTPS origin 或无附加字段的回环 HTTP(S) origin",
        ))
    }
}

fn parse_mooglemail_endpoint(value: &str) -> Option<Url> {
    if !valid_text(value, 2048) {
        return None;
    }
    let endpoint = Url::parse(value).ok()?;
    let path_is_root = endpoint.path().is_empty() || endpoint.path() == "/";
    let common = endpoint.host_str().is_some()
        && endpoint.username().is_empty()
        && endpoint.password().is_none()
        && path_is_root
        && endpoint.query().is_none()
        && endpoint.fragment().is_none();
    if !common {
        return None;
    }
    if endpoint.scheme() == "https"
        && endpoint.host_str().is_some_and(|host| host.eq_ignore_ascii_case("hentaiverse.org"))
        && endpoint.port().is_none()
    {
        return Some(endpoint);
    }
    if !matches!(endpoint.scheme(), "http" | "https") {
        return None;
    }
    let loopback = match endpoint.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    loopback.then_some(endpoint)
}

/// 边界规则由应用层唯一持有，Admin 保存 runtime 文档时使用同一函数，避免两侧再次分叉。
pub(crate) fn validate_mooglemail_equipment_filter(
    config: &MoogleMailEquipmentFilterConfig,
) -> Result<(), ConfigError> {
    let token_lists = [
        &config.qualities,
        &config.prefixes,
        &config.suffixes,
        &config.categories,
        &config.equipment_types,
    ]
    .map(Vec::as_slice);
    // 品质门槛始终存在（空列表也回退默认 L/P），因此不再要求其它过滤维度。
    let message =
        match validate_equipment_filter_bounds(config.level_min, config.level_max, token_lists) {
            Ok(()) => return Ok(()),
            Err(EquipmentFilterBoundsError::Level) => {
                "equipment_filter 的等级边界必须位于 0 至 1000；0 表示不限制，非零 max 不能小于 min"
            }
            Err(EquipmentFilterBoundsError::TooManyTokens) => {
                "equipment_filter 每个 token 列表最多 256 项"
            }
            Err(EquipmentFilterBoundsError::InvalidToken) => {
                "equipment_filter token 必须是唯一、非空且不含控制字符的文本"
            }
        };
    Err(ConfigError::InvalidMoogleMailConfig(message))
}

pub(crate) fn validate_mooglemail_world(
    world_id: &str,
    mailbox_key: &str,
    sender_allowlist: &[String],
    gateway_enabled: bool,
) -> Result<(), ConfigError> {
    if !valid_text(mailbox_key, 256) {
        return Err(ConfigError::InvalidMoogleMailConfig("mailbox_key 无效"));
    }
    if !matches!(world_id, "main" | "isekai") {
        return Err(ConfigError::InvalidMoogleMailConfig("world_id 只能是 main 或 isekai"));
    }
    if sender_allowlist.len() > 1000 {
        return Err(ConfigError::InvalidMoogleMailConfig("sender_allowlist 最多 1000 项"));
    }
    // 邮件关闭时沿用原文判重，避免未装配 gateway 的旧部署升级后无法启动。
    let has_duplicates = if gateway_enabled {
        sender_allowlist_has_case_duplicates(sender_allowlist)
    } else {
        let mut seen = HashSet::with_capacity(sender_allowlist.len());
        sender_allowlist.iter().any(|sender| !seen.insert(sender.as_str()))
    };
    if has_duplicates || sender_allowlist.iter().any(|sender| !valid_text(sender, 128)) {
        return Err(ConfigError::InvalidMoogleMailConfig("sender_allowlist 含有重复或无效发件人"));
    }
    Ok(())
}

/// 比较键不进入配置、指纹或 DTO。
pub(crate) fn sender_allowlist_has_case_duplicates(senders: &[String]) -> bool {
    let mut keys = HashSet::with_capacity(senders.len());
    // 与 gateway 装配时的判重同为 ASCII 忽略大小写，修改任一处须同步另一处。
    senders.iter().any(|sender| !keys.insert(sender.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    #[test]
    fn diagnostic_directory_needs_a_name_when_mail_uses_it() {
        let mut config = MoogleMailWorkerConfig::default();
        config.diagnostics.enabled = true;
        config.diagnostics.directory = "/".into();
        assert!(validate_mooglemail_worker(&config).is_ok());
        config.enabled = true;
        assert_matches!(
            validate_mooglemail_worker(&config),
            Err(ConfigError::InvalidMoogleMailConfig(reason))
                if reason.contains("diagnostics.directory")
        );
    }

    #[test]
    fn notification_requires_usable_mail_keys_and_cookie() {
        let config = MoogleMailWorkerConfig::default();
        let mut secrets = AppSecrets::from_file(
            toml::from_str(
                r#"mooglemail_cookie = "fixture=value"
equipment_data_key_id = "equipment-fixture"
equipment_data_key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
user_login_data_key_id = "login-fixture"
user_login_data_key = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBE="
"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(validate_notification_mail_credentials(&config, &secrets).is_ok());
        secrets.mooglemail_cookie =
            Some(SecretString::new("mooglemail_cookie", "a".repeat(4097)).unwrap());
        assert_matches!(
            validate_notification_mail_credentials(&config, &secrets),
            Err(ConfigError::InvalidMoogleMailConfig("mooglemail_cookie 格式无效"))
        );
        secrets.equipment_data_key = Some(
            SecretString::new(
                "equipment_data_key",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB=".to_owned(),
            )
            .unwrap(),
        );
        assert_matches!(
            validate_notification_mail_credentials(&config, &secrets),
            Err(ConfigError::InvalidDataKeyRing { domain: "equipment", .. })
        );
    }

    #[test]
    fn removed_mail_template_section_is_rejected_with_an_actionable_message() {
        // 通知模板已迁入数据库：Worker 配置里的旧区块不再读取，出现即拒绝并指明处理办法，
        // 不让运营误以为文件里的正文仍在生效。
        let config: MoogleMailWorkerConfig = toml::from_str(
            "[mail_templates]\nsettlement_seller_payout = \"Amount: {amount} {reference}\"\n",
        )
        .expect("旧区块本身必须能解析，才能给出明确的错误");
        assert!(config.removed_mail_templates);
        let error = validate_mooglemail_worker(&config).expect_err("旧区块必须被拒绝");
        assert_matches!(
            error,
            ConfigError::InvalidMoogleMailConfig(message)
                if message.contains("mail_templates") && message.contains("数据库")
        );
        // 没有该区块的配置不受影响；区块里的内容也不保留。
        assert!(!MoogleMailWorkerConfig::default().removed_mail_templates);
    }

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
            assert_matches!(
                error,
                ConfigError::InvalidMoogleMailConfig(message)
                    if message.contains("mail_scan_daily_full_interval_seconds")
            );
        }
    }

    #[test]
    fn runtime_equipment_tokens_accepted_by_admin_pass_the_worker_loader() {
        use auction_application::admin::{
            AdminRuntimeDocument, AdminRuntimeEquipmentFilterConfig, AdminRuntimeWorldsConfig,
        };

        use crate::runtime::validate_runtime_document;
        use crate::settings::WorkerConfig;
        use crate::settings::worker::apply_runtime_document;

        // 契约：Admin 保存 runtime 文档时接受的装备过滤 token，投影到 Worker 后必须通过启动校验，
        // 不能出现“保存成功、下次重启才失败”；含控制字符的 token 两侧都拒绝。
        let document = |token: &str| AdminRuntimeDocument {
            worlds: Some(AdminRuntimeWorldsConfig::default()),
            equipment_filter: Some(AdminRuntimeEquipmentFilterConfig {
                enabled: true,
                prefixes: vec![token.to_owned()],
                ..AdminRuntimeEquipmentFilterConfig::default()
            }),
            ..AdminRuntimeDocument::default()
        };
        let worker_accepts = |document: AdminRuntimeDocument| {
            let mut worker = WorkerConfig::default();
            apply_runtime_document(&mut worker, document);
            validate_mooglemail_worker(&worker.mooglemail).is_ok()
        };
        let now = Timestamp::now();
        assert!(validate_runtime_document(&document("Savage"), now).is_ok());
        for token in ["Savage", "Ethereal", "A\u{200B}B"] {
            if validate_runtime_document(&document(token), now).is_ok() {
                assert!(worker_accepts(document(token)), "{token:?}");
            }
        }
        for token in ["Sav\tage", "Leg\u{7}endary", "A\u{85}B"] {
            assert!(validate_runtime_document(&document(token), now).is_err(), "{token:?}");
            assert!(!worker_accepts(document(token)), "{token:?}");
        }
    }
}
