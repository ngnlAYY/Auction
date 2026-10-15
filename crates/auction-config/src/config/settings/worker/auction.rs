use serde::Deserialize;

use super::{
    control::WorkerControlConfig, delivery::DeliveryWorkerConfig, forum::ForumSyncWorkerConfig,
    fulfillment::FulfillmentWorkerConfig, mooglemail::MoogleMailWorkerConfig,
    operations_alert::OperationsAlertWorkerConfig, recovery::RecoveryWorkerConfig,
};

/// Auction 主循环及其它兼容轮询 owner 的共享调度边界。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerConfig {
    /// Worker control Unix socket 配置；默认关闭，避免无权限环境启动失败。
    pub control: WorkerControlConfig,
    /// Auction 主循环并发数；省略时为 8。
    pub concurrency: Option<u16>,
    /// Auction 主循环及兼容 owner 的轮询间隔秒数；省略时为 60 秒。
    pub poll_interval_seconds: Option<u64>,
    /// Auction 生命周期和结算各自的单轮批次上限；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// Auction 单个生命周期或结算任务的总处理超时秒数；默认 30。
    pub item_timeout_seconds: u64,
    /// Recovery 各阶段的持久化重试间隔秒数；省略时为 60 秒，最大不超过 86400 秒。
    pub recovery_retry_delay_seconds: u64,
    /// Delivery loop 的独立批次、lease 和超时边界。
    pub delivery: DeliveryWorkerConfig,
    /// Fulfillment loop 的独立批次、并发、lease 和超时边界。
    pub fulfillment: FulfillmentWorkerConfig,
    /// Recovery loop 的独立批次、并发、lease 和超时边界。
    pub recovery: RecoveryWorkerConfig,
    /// 可选 MoogleMail 轮询配置；省略时关闭远程读取。
    pub mooglemail: MoogleMailWorkerConfig,
    /// 可选 E-Hentai 论坛主题同步配置；省略时关闭远程写入。
    pub forum: ForumSyncWorkerConfig,
    /// 可选 operations alert 运维告警配置；省略时关闭告警扫描与 MoogleMail 通知。
    pub operations_alert: OperationsAlertWorkerConfig,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            control: WorkerControlConfig::default(),
            concurrency: None,
            poll_interval_seconds: None,
            batch_limit: 100,
            jitter_seconds: 5,
            item_timeout_seconds: 30,
            recovery_retry_delay_seconds: 60,
            delivery: DeliveryWorkerConfig::default(),
            fulfillment: FulfillmentWorkerConfig::default(),
            recovery: RecoveryWorkerConfig::default(),
            mooglemail: MoogleMailWorkerConfig::default(),
            forum: ForumSyncWorkerConfig::default(),
            operations_alert: OperationsAlertWorkerConfig::default(),
        }
    }
}
