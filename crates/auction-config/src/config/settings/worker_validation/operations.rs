use super::super::{
    ConfigError, MoogleMailWorkerConfig, OperationsAlertWorkerConfig, valid_config_text,
};

pub(crate) fn validate_operations_alert_worker(
    config: &OperationsAlertWorkerConfig,
    mooglemail: &MoogleMailWorkerConfig,
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
    if !matches!(config.world_id.as_str(), "main" | "isekai") {
        return Err(ConfigError::InvalidOperationsAlertConfig("world_id 只能是 main 或 isekai"));
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
    if config.notify_enabled()
        && config.world_id != mooglemail.world_id
        && !mooglemail.additional_worlds.iter().any(|world| world.world_id == config.world_id)
    {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警通知时 world_id 必须位于已启用的 MoogleMail worlds",
        ));
    }
    if config.notify_enabled() && !valid_config_text(&config.recipient, 256) {
        return Err(ConfigError::InvalidOperationsAlertConfig(
            "启用告警通知时 recipient 必须是非空且不含控制字符的文本",
        ));
    }
    if !config.recipient.is_empty() && !valid_config_text(&config.recipient, 256) {
        return Err(ConfigError::InvalidOperationsAlertConfig("recipient 无效"));
    }
    Ok(())
}
