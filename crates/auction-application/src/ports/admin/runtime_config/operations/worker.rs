//! Worker 调度候选与互相关联的吞吐、租约、超时约束。

use serde::{Deserialize, Serialize};

use super::{AdminRuntimeConfigError, require};

/// Worker 主循环及三个独立任务循环的启动参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeWorkerConfig {
    /// 是否由管理页覆盖全部四类 Worker 循环参数，默认关闭。
    pub managed: bool,
    /// 并发数。
    pub concurrency: u16,
    /// 轮询间隔秒数。
    pub poll_interval_seconds: u64,
    /// 每轮批量上限。
    pub batch_limit: u16,
    /// 轮询随机抖动秒数。
    pub jitter_seconds: u64,
    /// 单项处理超时秒数。
    pub item_timeout_seconds: u64,
    /// 恢复任务持久化重试间隔秒数。
    pub recovery_retry_delay_seconds: u64,
    /// 投递循环候选。
    pub delivery: AdminRuntimeDeliveryConfig,
    /// 履约循环候选。
    pub fulfillment: AdminRuntimeFulfillmentConfig,
    /// 恢复循环候选。
    pub recovery: AdminRuntimeRecoveryConfig,
}

impl Default for AdminRuntimeWorkerConfig {
    fn default() -> Self {
        Self {
            managed: false,
            concurrency: 8,
            poll_interval_seconds: 60,
            batch_limit: 100,
            jitter_seconds: 5,
            item_timeout_seconds: 30,
            recovery_retry_delay_seconds: 60,
            delivery: AdminRuntimeDeliveryConfig::default(),
            fulfillment: AdminRuntimeFulfillmentConfig::default(),
            recovery: AdminRuntimeRecoveryConfig::default(),
        }
    }
}

/// 投递循环的有界候选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeDeliveryConfig {
    /// 每轮投递批量上限。
    pub batch_limit: u16,
    /// 投递并发上限，实际值仍受 transport 约束。
    pub concurrency: u16,
    /// 投递轮询间隔秒数。
    pub poll_interval_seconds: u64,
    /// 投递轮询抖动秒数。
    pub jitter_seconds: u64,
    /// 投递 claim 租约秒数。
    pub lease_duration_seconds: u64,
    /// 发送期间续租间隔秒数。
    pub renew_interval_seconds: u64,
    /// 发送结果写回宽限秒数。
    pub sending_grace_seconds: u64,
    /// 完整投递请求超时秒数。
    pub request_timeout_seconds: u64,
    /// 投递终态写回超时秒数。
    pub finalize_timeout_seconds: u64,
}

impl Default for AdminRuntimeDeliveryConfig {
    fn default() -> Self {
        Self {
            batch_limit: 100,
            concurrency: 8,
            poll_interval_seconds: 60,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            renew_interval_seconds: 10,
            sending_grace_seconds: 120,
            request_timeout_seconds: 30,
            finalize_timeout_seconds: 5,
        }
    }
}

/// 履约循环的有界候选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeFulfillmentConfig {
    /// 每轮履约批量上限。
    pub batch_limit: u16,
    /// 履约并发上限。
    pub concurrency: u16,
    /// 履约轮询间隔秒数。
    pub poll_interval_seconds: u64,
    /// 履约轮询抖动秒数。
    pub jitter_seconds: u64,
    /// 履约 claim 租约秒数。
    pub lease_duration_seconds: u64,
    /// 单项履约超时秒数。
    pub item_timeout_seconds: u64,
    /// 履约持久化重试间隔秒数。
    pub retry_delay_seconds: u64,
}

impl Default for AdminRuntimeFulfillmentConfig {
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

/// 恢复循环的有界候选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeRecoveryConfig {
    /// 每阶段恢复批量上限。
    pub batch_limit: u16,
    /// 恢复 action 并发上限。
    pub concurrency: u16,
    /// 恢复轮询间隔秒数。
    pub poll_interval_seconds: u64,
    /// 恢复轮询抖动秒数。
    pub jitter_seconds: u64,
    /// 恢复 action 租约秒数。
    pub lease_duration_seconds: u64,
    /// 单项恢复超时秒数。
    pub item_timeout_seconds: u64,
    /// 余额低水位阈值，单位 Credits。
    pub low_balance_threshold: i64,
    /// 余额健康信息刷新间隔秒数。
    pub health_refresh_interval_seconds: u64,
    /// 余额证据陈旧阈值秒数。
    pub evidence_stale_after_seconds: u64,
}

impl Default for AdminRuntimeRecoveryConfig {
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

impl AdminRuntimeWorkerConfig {
    pub(super) fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        validate_cycle(
            self.batch_limit,
            self.concurrency,
            self.poll_interval_seconds,
            self.jitter_seconds,
            "worker",
        )?;
        let lease = (self.poll_interval_seconds * 3).max(30);
        require(
            (1..=86_400).contains(&self.item_timeout_seconds) && self.item_timeout_seconds < lease,
            "worker.item_timeout_seconds",
        )?;
        require(
            (1..=86_400).contains(&self.recovery_retry_delay_seconds),
            "worker.recovery_retry_delay_seconds",
        )?;
        self.delivery.validate()?;
        self.fulfillment.validate()?;
        self.recovery.validate()
    }
}

impl AdminRuntimeDeliveryConfig {
    fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        validate_cycle(
            self.batch_limit,
            self.concurrency,
            self.poll_interval_seconds,
            self.jitter_seconds,
            "worker.delivery",
        )?;
        require(
            (1..=86_400).contains(&self.lease_duration_seconds)
                && self.renew_interval_seconds > 0
                && self.renew_interval_seconds < self.lease_duration_seconds,
            "worker.delivery.renew_interval_seconds",
        )?;
        require(
            (1..=86_400).contains(&self.request_timeout_seconds)
                && self.sending_grace_seconds > self.request_timeout_seconds
                && self.sending_grace_seconds <= 86_400,
            "worker.delivery.sending_grace_seconds",
        )?;
        require(
            (1..=300).contains(&self.finalize_timeout_seconds),
            "worker.delivery.finalize_timeout_seconds",
        )
    }
}

impl AdminRuntimeFulfillmentConfig {
    fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        validate_cycle(
            self.batch_limit,
            self.concurrency,
            self.poll_interval_seconds,
            self.jitter_seconds,
            "worker.fulfillment",
        )?;
        validate_lease(
            self.lease_duration_seconds,
            self.item_timeout_seconds,
            "worker.fulfillment",
        )?;
        require(
            (1..=86_400).contains(&self.retry_delay_seconds),
            "worker.fulfillment.retry_delay_seconds",
        )
    }
}

impl AdminRuntimeRecoveryConfig {
    fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        validate_cycle(
            self.batch_limit,
            self.concurrency,
            self.poll_interval_seconds,
            self.jitter_seconds,
            "worker.recovery",
        )?;
        validate_lease(self.lease_duration_seconds, self.item_timeout_seconds, "worker.recovery")?;
        // 新管理文档必须能够经浏览器 JSON 精确往返；不能把大整数舍入成不同阈值。
        require(
            (0..=9_007_199_254_740_991).contains(&self.low_balance_threshold),
            "worker.recovery.low_balance_threshold",
        )?;
        require(
            (1..=86_400).contains(&self.health_refresh_interval_seconds)
                && self.evidence_stale_after_seconds >= self.health_refresh_interval_seconds
                && self.evidence_stale_after_seconds <= 86_400,
            "worker.recovery.evidence_stale_after_seconds",
        )
    }
}

fn validate_cycle(
    batch: u16,
    concurrency: u16,
    poll: u64,
    jitter: u64,
    field: &str,
) -> Result<(), AdminRuntimeConfigError> {
    require(
        (1..=1000).contains(&batch)
            && (1..=batch).contains(&concurrency)
            && (1..=86_400).contains(&poll)
            && jitter <= 86_400,
        field,
    )
}

fn validate_lease(lease: u64, timeout: u64, field: &str) -> Result<(), AdminRuntimeConfigError> {
    require((1..=86_400).contains(&lease) && timeout > 0 && timeout < lease, field)
}
