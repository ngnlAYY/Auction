//! 结算履约 Worker 配置的边界校验。

use super::super::{ConfigError, FulfillmentWorkerConfig};

pub(crate) fn validate_fulfillment_worker(
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
    if config.evidence_poll_seconds == 0 || config.evidence_poll_seconds > 86_400 {
        return Err(ConfigError::InvalidFulfillmentWorkerConfig(
            "evidence_poll_seconds 必须位于 1 至 86400",
        ));
    }
    Ok(())
}
