use serde::Deserialize;

/// Recovery Worker 的独立调度和批次边界。
/// 旧版 recovery loop 使用独立的 `batch_limit` 和 `concurrency`，不能复用拍卖主循环的
/// 全局并发，否则一个高峰期会同时放大退回、action、重列和退款处理量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryWorkerConfig {
    /// 每个 recovery phase 单轮最多领取的记录数；范围 1-1000，默认 100。
    pub batch_limit: u16,
    /// recovery action wave 的并发数；范围 1-batch_limit，默认 8。
    pub concurrency: u16,
    /// 两轮五阶段处理之间的固定轮询秒数；范围 1-86400，默认 60。
    pub poll_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；范围 0-86400，默认 5。
    pub jitter_seconds: u64,
    /// recovery action claim lease 秒数；范围 1-86400，默认 60。
    pub lease_duration_seconds: u64,
    /// 单个 recovery 记录或 action 的总处理超时秒数；必须小于 lease，默认 30。
    pub item_timeout_seconds: u64,
    /// 余额低水位阈值；低于该值时写入 `low`，默认 100 credits。
    pub low_balance_threshold: i64,
    /// recovery health 两次远端余额读取之间的最小间隔秒数；默认 60 秒。
    pub health_refresh_interval_seconds: u64,
    /// 外部余额证据超过该年龄后标记为 stale；必须不小于刷新间隔，默认 300 秒。
    pub evidence_stale_after_seconds: u64,
}

impl Default for RecoveryWorkerConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            item_timeout_seconds: 30,
            low_balance_threshold: 100,
            health_refresh_interval_seconds: 60,
            evidence_stale_after_seconds: 300,
        }
    }
}
