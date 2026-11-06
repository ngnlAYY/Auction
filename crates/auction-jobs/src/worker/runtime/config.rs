//! Worker 各业务循环的运行时边界配置；二次校验保护装配根与运行时之间的窄接口，
//! 拒绝无效租约、并发或轮询参数。

use std::{sync::Arc, time::Duration};

use auction_application::admin::AdminMailPort;
use auction_application::auction::AuctionPolicyTemplate;
use auction_domain::AuctionEarlyBirdCalendar;

/// 每周拍卖轮次生成所需的已解析配置。
#[derive(Clone)]
pub struct AuctionScheduleRuntimeConfig {
    pub enabled: bool,
    pub timezone: String,
    /// 周一至周日编号 0..=6。
    pub preview_weekday: u8,
    pub preview_minute: u16,
    pub open_weekday: u8,
    pub open_minute: u16,
    pub end_weekday: u8,
    pub end_minute: u16,
    pub worlds: Vec<String>,
    pub policy: AuctionPolicyTemplate,
    /// 已启用的赛季早鸟日历；替代被覆盖的周时段并控制赛季前收件暂停。
    pub early_bird: Option<AuctionEarlyBirdCalendar>,
    /// Mail owner 由装配根注入；调度器只通过窄端口改变暂停状态。
    pub mail: Option<Arc<dyn AdminMailPort>>,
    pub mailboxes: Vec<(String, String)>,
}

impl AuctionScheduleRuntimeConfig {
    pub(super) fn validate(&self) -> bool {
        self.enabled
            && !self.timezone.trim().is_empty()
            && self.preview_weekday <= 6
            && self.open_weekday <= 6
            && self.end_weekday <= 6
            && self.preview_minute < 24 * 60
            && self.open_minute < 24 * 60
            && self.end_minute < 24 * 60
            && !self.worlds.is_empty()
            && self.worlds.len() <= 2
            && self.worlds.iter().all(|world| matches!(world.as_str(), "main" | "isekai"))
            && self.worlds.windows(2).all(|pair| pair[0] != pair[1])
            && self.policy.validate().is_ok()
    }
}

/// 轮询、抖动、租约与重试类时长的统一上限。
const MAX_INTERVAL: Duration = Duration::from_secs(86_400);

/// 批量领取类循环共用的批量、并发、轮询与抖动边界。
fn valid_claim_loop(
    batch_limit: u16,
    concurrency: u16,
    poll_interval: Duration,
    jitter: Duration,
) -> bool {
    (1..=1000).contains(&batch_limit)
        && (1..=batch_limit).contains(&concurrency)
        && !poll_interval.is_zero()
        && poll_interval <= MAX_INTERVAL
        && jitter <= MAX_INTERVAL
}

/// 非零且不超过统一上限的时长。
fn bounded(value: Duration) -> bool {
    !value.is_zero() && value <= MAX_INTERVAL
}

#[derive(Debug, Clone, Copy)]
pub struct DeliveryWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub poll_interval: Duration,
    pub jitter: Duration,
    pub lease_duration: Duration,
    pub renew_interval: Duration,
    pub sending_grace: Duration,
    pub request_timeout: Duration,
    pub finalize_timeout: Duration,
}

impl Default for DeliveryWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            poll_interval: Duration::from_secs(60),
            jitter: Duration::from_secs(5),
            lease_duration: Duration::from_secs(60),
            renew_interval: Duration::from_secs(10),
            sending_grace: Duration::from_secs(120),
            request_timeout: Duration::from_secs(30),
            finalize_timeout: Duration::from_secs(5),
        }
    }
}

impl DeliveryWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        // 并发由 transport 的 `maximum_concurrency` 唯一决定，不再来自配置。
        (1..=1000).contains(&self.batch_limit)
            && !self.poll_interval.is_zero()
            && self.poll_interval <= MAX_INTERVAL
            && self.jitter <= MAX_INTERVAL
            && bounded(self.lease_duration)
            && !self.renew_interval.is_zero()
            && self.renew_interval < self.lease_duration
            && bounded(self.request_timeout)
            && self.sending_grace > self.request_timeout
            && self.sending_grace <= MAX_INTERVAL
            && !self.finalize_timeout.is_zero()
            && self.finalize_timeout <= Duration::from_secs(300)
    }
}

/// Auction 生命周期与结算 owner 的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct AuctionWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: Duration,
    pub jitter: Duration,
    pub lease_duration: Duration,
    pub item_timeout: Duration,
}

impl Default for AuctionWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: Duration::from_secs(60),
            jitter: Duration::from_secs(5),
            lease_duration: Duration::from_secs(180),
            item_timeout: Duration::from_secs(30),
        }
    }
}

impl AuctionWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        // 生命周期租约只要求非零，不套用统一上限，保持既有配置边界。
        valid_claim_loop(self.batch_limit, self.concurrency, self.poll_interval, self.jitter)
            && !self.lease_duration.is_zero()
            && !self.item_timeout.is_zero()
            && self.item_timeout < self.lease_duration
    }
}

/// Forum 主题发布与回帖导入 owner 的独立运行边界。
#[derive(Debug, Clone, Copy)]
pub struct ForumWorkerRuntimeConfig {
    pub poll_interval: Duration,
    pub jitter: Duration,
    pub lease_duration: Duration,
}

impl Default for ForumWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(300),
            jitter: Duration::from_secs(5),
            lease_duration: Duration::from_secs(300),
        }
    }
}

impl ForumWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        bounded(self.poll_interval) && self.jitter <= MAX_INTERVAL && bounded(self.lease_duration)
    }

    /// 只替换热读取的轮询与抖动；租约仍来自启动配置。
    pub(super) fn with_hot(self, hot: &auction_application::admin::WorkerForumHotConfig) -> Self {
        Self {
            poll_interval: Duration::from_secs(hot.poll_interval_seconds),
            jitter: Duration::from_secs(hot.jitter_seconds),
            lease_duration: self.lease_duration,
        }
    }
}

impl DeliveryWorkerRuntimeConfig {
    /// 只替换热读取的批次、轮询、抖动与超时；租约、续租、发送宽限和并发仍来自启动配置。
    pub(super) fn with_hot(
        self,
        hot: &auction_application::admin::WorkerDeliveryHotConfig,
    ) -> Self {
        Self {
            batch_limit: hot.batch_limit,
            poll_interval: Duration::from_secs(hot.poll_interval_seconds),
            jitter: Duration::from_secs(hot.jitter_seconds),
            request_timeout: Duration::from_secs(hot.request_timeout_seconds),
            finalize_timeout: Duration::from_secs(hot.finalize_timeout_seconds),
            ..self
        }
    }
}

/// Fulfillment 与 Recovery loop 共用的已校验运行边界；两者字段、默认值与校验完全一致。
#[derive(Debug, Clone, Copy)]
pub struct FulfillmentWorkerRuntimeConfig {
    pub batch_limit: u16,
    pub concurrency: u16,
    pub poll_interval: Duration,
    pub jitter: Duration,
    pub lease_duration: Duration,
    pub item_timeout: Duration,
    pub retry_delay: Duration,
}

/// Recovery loop 复用 Fulfillment 的运行边界。
pub type RecoveryWorkerRuntimeConfig = FulfillmentWorkerRuntimeConfig;

impl Default for FulfillmentWorkerRuntimeConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval: Duration::from_secs(60),
            jitter: Duration::from_secs(5),
            lease_duration: Duration::from_secs(60),
            item_timeout: Duration::from_secs(30),
            retry_delay: Duration::from_secs(60),
        }
    }
}

impl FulfillmentWorkerRuntimeConfig {
    pub(super) fn validate(self) -> bool {
        valid_claim_loop(self.batch_limit, self.concurrency, self.poll_interval, self.jitter)
            && bounded(self.lease_duration)
            && !self.item_timeout.is_zero()
            && self.item_timeout < self.lease_duration
            && bounded(self.retry_delay)
    }
}

#[cfg(test)]
mod hot_overlay_tests {
    use std::time::Duration;

    use auction_application::admin::{WorkerDeliveryHotConfig, WorkerForumHotConfig};

    use super::{DeliveryWorkerRuntimeConfig, ForumWorkerRuntimeConfig};

    #[test]
    fn delivery_hot_overlay_keeps_lease_and_grace() {
        let startup = DeliveryWorkerRuntimeConfig {
            lease_duration: Duration::from_secs(90),
            renew_interval: Duration::from_secs(15),
            sending_grace: Duration::from_secs(400),
            ..DeliveryWorkerRuntimeConfig::default()
        };
        let hot = WorkerDeliveryHotConfig {
            batch_limit: 12,
            poll_interval_seconds: 7,
            jitter_seconds: 2,
            request_timeout_seconds: 200,
            finalize_timeout_seconds: 8,
        };
        let next = startup.with_hot(&hot);
        assert_eq!(next.batch_limit, 12);
        assert_eq!(next.poll_interval, Duration::from_secs(7));
        assert_eq!(next.jitter, Duration::from_secs(2));
        assert_eq!(next.request_timeout, Duration::from_secs(200));
        assert_eq!(next.finalize_timeout, Duration::from_secs(8));
        // 重启字段保持启动值，热快照不能改写 claim 或 sending 围栏。
        assert_eq!(next.lease_duration, startup.lease_duration);
        assert_eq!(next.renew_interval, startup.renew_interval);
        assert_eq!(next.sending_grace, startup.sending_grace);
        assert!(next.validate());
    }

    #[test]
    fn forum_hot_overlay_keeps_lease() {
        let startup = ForumWorkerRuntimeConfig {
            lease_duration: Duration::from_secs(300),
            ..Default::default()
        };
        let hot = WorkerForumHotConfig {
            batch_size: 9,
            poll_interval_seconds: 45,
            jitter_seconds: 3,
            item_timeout_seconds: 120,
        };
        let next = startup.with_hot(&hot);
        assert_eq!(next.poll_interval, Duration::from_secs(45));
        assert_eq!(next.jitter, Duration::from_secs(3));
        assert_eq!(next.lease_duration, Duration::from_secs(300));
    }
}
