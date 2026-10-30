use auction_application::AdminRuntimeDeliveryConfig;
use serde::Deserialize;

/// Delivery outbox 的独立领取、续租与外部调用边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeliveryWorkerConfig {
    /// 单轮最多领取的 delivery 数量；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// 单轮 delivery 并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// Delivery 独立轮询间隔秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// sending 期间续租间隔秒数；必须小于 lease，默认 10。
    pub renew_interval_seconds: u64,
    /// lease 过期后等待外部结果写回的宽限秒数；必须大于请求超时，默认 300。
    pub sending_grace_seconds: u64,
    /// 单次完整 DeliveryTransport 调用总超时秒数；默认 180。
    pub request_timeout_seconds: u64,
    /// 成功、失败或人工对账结果写回的超时秒数；默认 5。
    pub finalize_timeout_seconds: u64,
}

/// 部署 TOML 省略字段时与 runtime 接管模板使用同一组默认值，由 application 的 runtime 模型唯一持有。
impl Default for DeliveryWorkerConfig {
    fn default() -> Self {
        Self::from(&AdminRuntimeDeliveryConfig::default())
    }
}

impl From<&AdminRuntimeDeliveryConfig> for DeliveryWorkerConfig {
    fn from(delivery: &AdminRuntimeDeliveryConfig) -> Self {
        Self {
            batch_limit: delivery.batch_limit,
            concurrency: delivery.concurrency,
            poll_interval_seconds: delivery.poll_interval_seconds,
            jitter_seconds: delivery.jitter_seconds,
            lease_duration_seconds: delivery.lease_duration_seconds,
            renew_interval_seconds: delivery.renew_interval_seconds,
            sending_grace_seconds: delivery.sending_grace_seconds,
            request_timeout_seconds: delivery.request_timeout_seconds,
            finalize_timeout_seconds: delivery.finalize_timeout_seconds,
        }
    }
}
