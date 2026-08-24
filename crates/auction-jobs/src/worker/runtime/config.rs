//! Worker 各业务循环的运行时边界配置。
//!
//! 配置层负责 TOML 字段和跨字段校验，这里的二次校验保护装配根与运行时之间的窄接口，避免
//! 手动构造无效租约、并发或轮询参数。

/// Delivery loop 已经由配置层校验的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct DeliveryWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub renew_interval: std::time::Duration,
    pub sending_grace: std::time::Duration,
    pub request_timeout: std::time::Duration,
    pub finalize_timeout: std::time::Duration,
}

impl Default for DeliveryWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(60),
            renew_interval: std::time::Duration::from_secs(10),
            sending_grace: std::time::Duration::from_secs(120),
            request_timeout: std::time::Duration::from_secs(30),
            finalize_timeout: std::time::Duration::from_secs(5),
        }
    }
}

impl DeliveryWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        self.batch_limit > 0
            && self.batch_limit <= 1000
            && self.concurrency > 0
            && self.concurrency <= self.batch_limit
            && !self.poll_interval.is_zero()
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && !self.renew_interval.is_zero()
            && self.renew_interval < self.lease_duration
            && !self.request_timeout.is_zero()
            && self.sending_grace > self.request_timeout
            && !self.finalize_timeout.is_zero()
    }
}

/// Auction 生命周期与结算 owner 的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct AuctionWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub item_timeout: std::time::Duration,
}

impl Default for AuctionWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(180),
            item_timeout: std::time::Duration::from_secs(30),
        }
    }
}

impl AuctionWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        self.batch_limit > 0
            && self.batch_limit <= 1000
            && self.concurrency > 0
            && self.concurrency <= self.batch_limit
            && !self.poll_interval.is_zero()
            && self.poll_interval <= std::time::Duration::from_secs(86_400)
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && !self.item_timeout.is_zero()
            && self.item_timeout < self.lease_duration
    }
}

/// Forum 主题发布与回帖导入 owner 的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct ForumWorkerRuntimeConfig {
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
}

impl Default for ForumWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(300),
        }
    }
}

impl ForumWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        !self.poll_interval.is_zero()
            && self.poll_interval <= std::time::Duration::from_secs(86_400)
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && self.lease_duration <= std::time::Duration::from_secs(86_400)
    }
}

/// Fulfillment loop 已经由配置层校验的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct FulfillmentWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub item_timeout: std::time::Duration,
    pub retry_delay: std::time::Duration,
}

impl Default for FulfillmentWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(60),
            item_timeout: std::time::Duration::from_secs(30),
            retry_delay: std::time::Duration::from_secs(60),
        }
    }
}

impl FulfillmentWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        self.batch_limit > 0
            && self.batch_limit <= 1000
            && self.concurrency > 0
            && self.concurrency <= self.batch_limit
            && !self.poll_interval.is_zero()
            && self.poll_interval <= std::time::Duration::from_secs(86_400)
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && self.lease_duration <= std::time::Duration::from_secs(86_400)
            && !self.item_timeout.is_zero()
            && self.item_timeout < self.lease_duration
            && !self.retry_delay.is_zero()
            && self.retry_delay <= std::time::Duration::from_secs(86_400)
    }
}

/// Recovery loop 已经由配置层校验的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct RecoveryWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: std::time::Duration,
    pub jitter: std::time::Duration,
    pub lease_duration: std::time::Duration,
    pub item_timeout: std::time::Duration,
    pub retry_delay: std::time::Duration,
}

impl Default for RecoveryWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: std::time::Duration::from_secs(60),
            jitter: std::time::Duration::from_secs(5),
            lease_duration: std::time::Duration::from_secs(60),
            item_timeout: std::time::Duration::from_secs(30),
            retry_delay: std::time::Duration::from_secs(60),
        }
    }
}

impl RecoveryWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        self.batch_limit > 0
            && self.batch_limit <= 1000
            && self.concurrency > 0
            && self.concurrency <= self.batch_limit
            && !self.poll_interval.is_zero()
            && self.poll_interval <= std::time::Duration::from_secs(86_400)
            && self.jitter <= std::time::Duration::from_secs(86_400)
            && !self.lease_duration.is_zero()
            && self.lease_duration <= std::time::Duration::from_secs(86_400)
            && !self.item_timeout.is_zero()
            && self.item_timeout < self.lease_duration
            && !self.retry_delay.is_zero()
            && self.retry_delay <= std::time::Duration::from_secs(86_400)
    }
}
