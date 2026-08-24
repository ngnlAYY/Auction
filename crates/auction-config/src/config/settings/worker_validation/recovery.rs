use super::*;

pub(crate) fn validate_recovery_worker(config: &RecoveryWorkerConfig) -> Result<(), ConfigError> {
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
