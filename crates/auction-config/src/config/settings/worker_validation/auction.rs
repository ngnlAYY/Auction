use super::*;

pub(crate) fn validate_auction_worker(
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
