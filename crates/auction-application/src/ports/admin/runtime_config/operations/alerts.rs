//! 运维告警候选，复用应用告警策略的批次、租约、阈值和收件人校验。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{OperationsAlertPolicy, OperationsAlertTarget};

use super::{AdminRuntimeConfigError, invalid, require};

/// 运维告警的非敏感运行参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeAlertsConfig {
    /// 是否由管理页覆盖运维告警参数，默认关闭。
    pub managed: bool,
    /// 是否启用告警扫描和通知。
    pub enabled: bool,
    /// 各告警循环的扫描间隔秒数。
    pub scan_interval_seconds: u64,
    /// 不大于扫描间隔的随机抖动秒数。
    pub jitter_seconds: u64,
    /// 健康来源读取超时秒数。
    pub source_timeout_seconds: u64,
    /// 同轮告警合并窗口秒数。
    pub batch_window_seconds: u64,
    /// 同类告警通知冷却秒数。
    pub cooldown_seconds: u64,
    /// 告警批次租约秒数，必须覆盖四倍来源读取超时。
    pub claim_lease_seconds: u64,
    /// 失败告警的持久化重试间隔秒数。
    pub retry_delay_seconds: u64,
    /// 每轮最多处理的告警信号数。
    pub max_signals_per_scan: usize,
    /// 单个通知批次最多信号数。
    pub max_batch_items: usize,
    /// 单条告警正文的最大字节数。
    pub max_body_bytes: usize,
    /// 待投递数量阈值。
    pub delivery_pending_count_threshold: i64,
    /// 待投递最老记录年龄阈值秒数。
    pub delivery_pending_age_seconds: u64,
    /// 投递人工对账数量阈值。
    pub delivery_manual_reconcile_count_threshold: i64,
    /// 履约积压数量阈值。
    pub fulfillment_backlog_count_threshold: i64,
    /// 履约积压年龄阈值秒数。
    pub fulfillment_backlog_age_seconds: u64,
    /// 恢复付款逾期数量阈值。
    pub recovery_overdue_count_threshold: i64,
    /// 恢复付款逾期年龄阈值秒数。
    pub recovery_overdue_age_seconds: u64,
    /// 通知使用的游戏世界，只允许 main 或 isekai。
    pub world_id: String,
    /// 通知收件人，启用并接管告警时必填。
    pub recipient: String,
}

impl Default for AdminRuntimeAlertsConfig {
    fn default() -> Self {
        Self {
            managed: false,
            enabled: false,
            scan_interval_seconds: 60,
            jitter_seconds: 5,
            source_timeout_seconds: 5,
            batch_window_seconds: 120,
            cooldown_seconds: 1_800,
            claim_lease_seconds: 120,
            retry_delay_seconds: 300,
            max_signals_per_scan: 64,
            max_batch_items: 20,
            max_body_bytes: 4 * 1024,
            delivery_pending_count_threshold: 1,
            delivery_pending_age_seconds: 900,
            delivery_manual_reconcile_count_threshold: 1,
            fulfillment_backlog_count_threshold: 1,
            fulfillment_backlog_age_seconds: 900,
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_seconds: 900,
            world_id: "main".to_owned(),
            recipient: String::new(),
        }
    }
}

impl AdminRuntimeAlertsConfig {
    pub(super) fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        require(
            (1..=86_400).contains(&self.scan_interval_seconds)
                && self.jitter_seconds <= self.scan_interval_seconds,
            "alerts.scan_interval_seconds",
        )?;
        require((1..=30).contains(&self.source_timeout_seconds), "alerts.source_timeout_seconds")?;
        for count in [
            self.delivery_pending_count_threshold,
            self.delivery_manual_reconcile_count_threshold,
            self.fulfillment_backlog_count_threshold,
            self.recovery_overdue_count_threshold,
        ] {
            require((1..=9_007_199_254_740_991).contains(&count), "alerts.count_threshold")?;
        }
        OperationsAlertPolicy {
            source_timeout: Duration::from_secs(self.source_timeout_seconds),
            batch_window: Duration::from_secs(self.batch_window_seconds),
            cooldown: Duration::from_secs(self.cooldown_seconds),
            claim_lease: Duration::from_secs(self.claim_lease_seconds),
            retry_delay: Duration::from_secs(self.retry_delay_seconds),
            max_signals_per_scan: self.max_signals_per_scan,
            max_batch_items: self.max_batch_items,
            max_body_bytes: self.max_body_bytes,
            delivery_pending_count_threshold: self.delivery_pending_count_threshold,
            delivery_pending_age_threshold: Duration::from_secs(self.delivery_pending_age_seconds),
            delivery_manual_reconcile_count_threshold: self
                .delivery_manual_reconcile_count_threshold,
            fulfillment_backlog_count_threshold: self.fulfillment_backlog_count_threshold,
            fulfillment_backlog_age_threshold: Duration::from_secs(
                self.fulfillment_backlog_age_seconds,
            ),
            recovery_overdue_count_threshold: self.recovery_overdue_count_threshold,
            recovery_overdue_age_threshold: Duration::from_secs(self.recovery_overdue_age_seconds),
        }
        .validate()
        .map_err(|_| invalid("alerts"))?;
        require(matches!(self.world_id.as_str(), "main" | "isekai"), "alerts.world_id")?;
        if self.managed && self.enabled || !self.recipient.is_empty() {
            OperationsAlertTarget {
                world_id: self.world_id.clone(),
                recipient: self.recipient.clone(),
            }
            .validate()
            .map_err(|_| invalid("alerts.recipient"))?;
        }
        Ok(())
    }
}
