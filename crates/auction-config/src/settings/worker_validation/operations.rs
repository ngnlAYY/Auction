//! 运维告警 Worker 配置的边界校验。

use auction_domain::valid_text;

use super::super::{ConfigError, MoogleMailWorkerConfig, OperationsAlertWorkerConfig, Role};
use crate::settings::secrets::AppSecrets;

pub(crate) fn validate_operations_alert_worker(
    role: Role,
    config: &OperationsAlertWorkerConfig,
    mooglemail: &MoogleMailWorkerConfig,
    secrets: Option<&AppSecrets>,
) -> Result<(), ConfigError> {
    if config.scan_interval_seconds == 0 || config.scan_interval_seconds > 86_400 {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "scan_interval_seconds 必须位于 1 至 86400",
        ));
    }
    if config.jitter_seconds > config.scan_interval_seconds {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "jitter_seconds 必须不大于 scan_interval_seconds",
        ));
    }
    let policy = config.policy();
    if policy.validate().is_err() {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "时间、阈值、批次数量或正文大小超出允许范围",
        ));
    }
    if config.enabled.is_some()
        && (config.scan_enabled.is_some() || config.notify_enabled.is_some())
    {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "enabled 是旧版兼容别名，不能与 scan_enabled/notify_enabled 同时出现",
        ));
    }
    if config.notify_enabled() && !config.scan_enabled() {
        return Err(ConfigError::InvalidOperationsAlertConfig("启用主动通知时必须同时启用扫描"));
    }
    if auction_application::operations::validate_webhook_template(&config.webhook_template).is_err()
    {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "webhook_template 必须是包含合法占位符的 JSON 对象模板",
        ));
    }
    if config.webhook_enabled() && !config.scan_enabled() {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用 webhook 通知时必须同时启用扫描",
        ));
    }
    if !matches!(config.world_id.as_str(), "main" | "isekai") {
        return Err(ConfigError::InvalidOperationsAlertConfig("world_id 只能是 main 或 isekai"));
    }
    if role == Role::Worker && config.notify_enabled() && !mooglemail.enabled {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警主动通知时必须启用 worker.mooglemail.enabled",
        ));
    }
    if config.notify_enabled()
        && !mooglemail.configured_mailboxes().any(|(world, _)| world == config.world_id)
    {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警通知时 world_id 必须位于已启用的 MoogleMail worlds",
        ));
    }
    if config.notify_enabled() && !valid_text(&config.recipient, 256) {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警通知时 recipient 必须是非空且不含控制字符的文本",
        ));
    }
    if !config.recipient.is_empty() && !valid_text(&config.recipient, 256) {
        return Err(ConfigError::InvalidOperationsAlertConfig("recipient 无效"));
    }
    let Some(secrets) = secrets.filter(|_| role == Role::Worker) else {
        return Ok(());
    };
    if (config.notify_enabled() || config.webhook_enabled()) && secrets.database_url.is_none() {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警通知时必须配置 PostgreSQL database_url",
        ));
    }
    if config.webhook_enabled() && secrets.operations_alert_webhook_url.is_none() {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警 webhook 时必须配置 operations_alert_webhook_url",
        ));
    }
    if config.notify_enabled() {
        super::mooglemail::validate_notification_mail_credentials(mooglemail, secrets)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::validate_operations_alert_worker;
    use crate::settings::secrets::{AppSecrets, SecretString};
    use crate::settings::{ConfigError, MoogleMailWorkerConfig, OperationsAlertWorkerConfig, Role};

    /// 长期未领取阈值与其他年龄阈值同一范围：1 秒至 30 天，越界在加载期失败关闭。
    #[test]
    fn collection_overdue_threshold_is_bounded() {
        let mail = MoogleMailWorkerConfig::default();
        let secrets = AppSecrets::default();
        for accepted in [1, 259_200, 2_592_000] {
            let config = OperationsAlertWorkerConfig {
                fulfillment_collection_overdue_age_seconds: accepted,
                ..OperationsAlertWorkerConfig::default()
            };
            assert!(
                validate_operations_alert_worker(Role::Worker, &config, &mail, Some(&secrets))
                    .is_ok(),
                "{accepted}"
            );
        }
        for rejected in [0, 2_592_001] {
            let config = OperationsAlertWorkerConfig {
                fulfillment_collection_overdue_age_seconds: rejected,
                ..OperationsAlertWorkerConfig::default()
            };
            assert_matches!(
                validate_operations_alert_worker(Role::Worker, &config, &mail, Some(&secrets)),
                Err(ConfigError::InvalidOperationsAlertConfig(_)),
                "{rejected}"
            );
        }
    }

    #[test]
    fn webhook_requires_database_and_address_only_when_enabled() {
        let mut config = OperationsAlertWorkerConfig {
            webhook_enabled: true,
            ..OperationsAlertWorkerConfig::default()
        };
        let mail = MoogleMailWorkerConfig::default();
        let mut secrets = AppSecrets::default();
        let check = |config: &OperationsAlertWorkerConfig, secrets: &AppSecrets| {
            validate_operations_alert_worker(Role::Worker, config, &mail, Some(secrets))
        };
        assert_matches!(
            check(&config, &secrets),
            Err(ConfigError::InvalidOperationsAlertConfig(reason)) if reason.contains("database_url")
        );
        secrets.database_url = Some(
            SecretString::new("database_url", "postgresql://localhost/fixture".to_owned()).unwrap(),
        );
        assert_matches!(
            check(&config, &secrets),
            Err(ConfigError::InvalidOperationsAlertConfig(reason))
                if reason.contains("operations_alert_webhook_url")
        );
        secrets.operations_alert_webhook_url = Some(
            SecretString::new(
                "operations_alert_webhook_url",
                "https://example.invalid/hook".to_owned(),
            )
            .unwrap(),
        );
        assert!(check(&config, &secrets).is_ok());
        config.webhook_enabled = false;
        assert!(
            validate_operations_alert_worker(
                Role::Worker,
                &config,
                &mail,
                Some(&AppSecrets::default())
            )
            .is_ok()
        );
    }
}
