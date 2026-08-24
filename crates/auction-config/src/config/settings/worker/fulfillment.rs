use serde::Deserialize;

/// Settlement fulfillment Worker 的独立领取与处理边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FulfillmentWorkerConfig {
    /// 单轮最多处理的 fulfillment 数量；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// fulfillment action wave 的并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// 两轮处理之间的固定轮询秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// fulfillment claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// 单个 fulfillment 的总处理超时秒数；必须小于 lease，默认 30。
    pub item_timeout_seconds: u64,
    /// 依赖暂不可用后的持久化重试间隔秒数；范围 1-86400，默认 60。
    pub retry_delay_seconds: u64,
}

impl Default for FulfillmentWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            item_timeout_seconds: 30,
            retry_delay_seconds: 60,
        }
    }
}
