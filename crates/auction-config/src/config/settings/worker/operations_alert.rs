use serde::Deserialize;

use auction_application::{OperationsAlertPolicy, OperationsAlertTarget};

/// Worker 使用的 operations alert 扫描、阈值和通知批次配置。
///
/// 所有字段都有默认值，因此 TOML 可以只覆盖需要调整的字段；启用告警时必须显式提供
/// 合法的 `recipient`。通知正文只由应用层从健康摘要生成，不接受配置注入。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperationsAlertWorkerConfig {
    /// 是否启用运维告警扫描和 MoogleMail 通知；省略时关闭。
    pub enabled: bool,
    /// scan、dispatch 与 reconcile 各自的轮询间隔秒数；范围 1-86400，省略时为 60 秒。
    pub scan_interval_seconds: u64,
    /// 读取四类健康来源的超时秒数，范围 1-30；省略时为 5 秒。
    pub source_timeout_seconds: u64,
    /// 同一轮观察结果合并到通知批次前的窗口秒数，范围 1-3600；省略时为 120 秒。
    pub batch_window_seconds: u64,
    /// 同一告警 signal 的通知冷却秒数，不能小于批次窗口；省略时为 1800 秒。
    pub cooldown_seconds: u64,
    /// 告警批次 claim 租约秒数，必须覆盖来源读取超时；省略时为 120 秒。
    pub claim_lease_seconds: u64,
    /// 告警投递失败后的持久化重试间隔秒数；省略时为 300 秒。
    pub retry_delay_seconds: u64,
    /// 单轮最多处理的 signal 数量，范围 1-64；省略时为 64。
    pub max_signals_per_scan: usize,
    /// 单个 MoogleMail 通知批次最多包含的 signal 数量，范围 1-64；省略时为 20。
    pub max_batch_items: usize,
    /// 单个 MoogleMail 通知正文的最大字节数，范围 256-16384；省略时为 4096。
    pub max_body_bytes: usize,
    /// delivery pending 数量阈值；达到后产生告警，省略时为 1。
    pub delivery_pending_count_threshold: i64,
    /// delivery pending 最老记录年龄阈值秒数，范围 1-2592000；省略时为 900 秒。
    pub delivery_pending_age_seconds: u64,
    /// delivery manual_reconcile 数量阈值；达到后产生告警，省略时为 1。
    pub delivery_manual_reconcile_count_threshold: i64,
    /// settlement fulfillment backlog 数量阈值；达到后产生告警，省略时为 1。
    pub fulfillment_backlog_count_threshold: i64,
    /// settlement fulfillment backlog 最老记录年龄阈值秒数，省略时为 900 秒。
    pub fulfillment_backlog_age_seconds: u64,
    /// recovery payment overdue 数量阈值；达到后产生告警，省略时为 1。
    pub recovery_overdue_count_threshold: i64,
    /// recovery payment overdue 最老记录年龄阈值秒数，省略时为 900 秒。
    pub recovery_overdue_age_seconds: u64,
    /// 运维告警通知使用的游戏世界，只允许 main 或 isekai；省略时为 main。
    pub world_id: String,
    /// 运维告警通知收件人；告警启用时必填，省略时为空字符串。
    pub recipient: String,
}

impl Default for OperationsAlertWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            scan_interval_seconds: 60,
            source_timeout_seconds: 5,
            batch_window_seconds: 2 * 60,
            cooldown_seconds: 30 * 60,
            claim_lease_seconds: 120,
            retry_delay_seconds: 5 * 60,
            max_signals_per_scan: 64,
            max_batch_items: 20,
            max_body_bytes: 4 << 10,
            delivery_pending_count_threshold: 1,
            delivery_pending_age_seconds: 15 * 60,
            delivery_manual_reconcile_count_threshold: 1,
            fulfillment_backlog_count_threshold: 1,
            fulfillment_backlog_age_seconds: 15 * 60,
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_seconds: 15 * 60,
            world_id: "main".to_owned(),
            recipient: String::new(),
        }
    }
}

impl OperationsAlertWorkerConfig {
    /// 将已校验的 TOML 设置转换为应用层告警策略。
    pub fn policy(&self) -> OperationsAlertPolicy {
        OperationsAlertPolicy {
            source_timeout: std::time::Duration::from_secs(self.source_timeout_seconds),
            batch_window: std::time::Duration::from_secs(self.batch_window_seconds),
            cooldown: std::time::Duration::from_secs(self.cooldown_seconds),
            claim_lease: std::time::Duration::from_secs(self.claim_lease_seconds),
            retry_delay: std::time::Duration::from_secs(self.retry_delay_seconds),
            max_signals_per_scan: self.max_signals_per_scan,
            max_batch_items: self.max_batch_items,
            max_body_bytes: self.max_body_bytes,
            delivery_pending_count_threshold: self.delivery_pending_count_threshold,
            delivery_pending_age_threshold: std::time::Duration::from_secs(
                self.delivery_pending_age_seconds,
            ),
            delivery_manual_reconcile_count_threshold: self
                .delivery_manual_reconcile_count_threshold,
            fulfillment_backlog_count_threshold: self.fulfillment_backlog_count_threshold,
            fulfillment_backlog_age_threshold: std::time::Duration::from_secs(
                self.fulfillment_backlog_age_seconds,
            ),
            recovery_overdue_count_threshold: self.recovery_overdue_count_threshold,
            recovery_overdue_age_threshold: std::time::Duration::from_secs(
                self.recovery_overdue_age_seconds,
            ),
        }
    }

    /// 将已校验的 TOML 设置转换为应用层告警收件目标。
    pub fn target(&self) -> OperationsAlertTarget {
        OperationsAlertTarget { world_id: self.world_id.clone(), recipient: self.recipient.clone() }
    }
}
