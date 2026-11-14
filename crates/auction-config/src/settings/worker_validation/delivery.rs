//! Delivery Worker 配置的边界校验。

use super::super::{ConfigError, DeliveryWorkerConfig};

pub(crate) fn validate_delivery_worker(config: &DeliveryWorkerConfig) -> Result<(), ConfigError> {
    if config.batch_limit == 0 || config.batch_limit > 1000 {
        return Err(ConfigError::InvalidDeliveryWorkerConfig("batch_limit 必须位于 1 至 1000"));
    }
    if config.obsolete_concurrency.is_some() {
        return Err(ConfigError::InvalidDeliveryWorkerConfig(
            "[worker.delivery] concurrency 已移除：MoogleMail transport 的有效并发始终为 1，请删除该字段",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_delivery_concurrency_is_rejected_with_an_actionable_message() {
        let legacy: DeliveryWorkerConfig =
            toml::from_str("concurrency = 8").expect("旧字段仍可解析");
        let error = validate_delivery_worker(&legacy).expect_err("旧字段必须被拒绝");
        assert!(
            matches!(error, ConfigError::InvalidDeliveryWorkerConfig(message) if message.contains("已移除"))
        );

        let current: DeliveryWorkerConfig =
            toml::from_str("batch_limit = 50\npoll_interval_seconds = 30").expect("新字段可解析");
        assert!(validate_delivery_worker(&current).is_ok());
    }
}
