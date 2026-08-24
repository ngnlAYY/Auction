use super::*;

pub(crate) fn validate_operations_alert_worker(
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
