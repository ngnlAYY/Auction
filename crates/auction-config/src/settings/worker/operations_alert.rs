use serde::Deserialize;

use auction_application::operations::{
    DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE, OperationsAlertPolicy, OperationsAlertTarget,
};

/// Worker 使用的 operations alert 扫描、阈值和通知批次配置。
///
/// 扫描/告警台与主动 MoogleMail 通知是两个独立能力：`scan_enabled` 只把来源观察写入
/// signal 与告警台，`notify_enabled` 才会创建运营邮件批次。旧字段 `enabled` 仅作为兼容
/// 别名同时设置两者，不能与新字段混用。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperationsAlertWorkerConfig {
    /// 旧版兼容别名：同时设置扫描与通知；省略时由两个新字段决定。
    pub enabled: Option<bool>,
    /// 是否执行来源扫描并写入告警台；省略时默认 true（有数据库时）。
    pub scan_enabled: Option<bool>,
    /// 是否创建 MoogleMail 运维通知批次；省略时默认 false。
    pub notify_enabled: Option<bool>,
    /// 是否启用独立 webhook 通道；省略时默认 false，与 MoogleMail 通知互不影响。
    pub webhook_enabled: bool,
    /// webhook JSON 模板；创建投递时冻结，支持 kind/scope/source/count/age_seconds 占位符。
    pub webhook_template: String,
    /// scan、dispatch 与 reconcile 各自的轮询间隔秒数；范围 1-86400，省略时为 60 秒。
    pub scan_interval_seconds: u64,
    /// 每轮完成后附加的随机抖动上限秒数；不能超过轮询间隔，省略时为 5 秒。
    pub jitter_seconds: u64,
    /// 读取全部告警来源的超时秒数，范围 1-30；省略时为 5 秒。
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
            enabled: None,
            scan_enabled: None,
            notify_enabled: None,
            webhook_enabled: false,
            webhook_template: DEFAULT_OPERATIONS_ALERT_WEBHOOK_TEMPLATE.to_owned(),
            scan_interval_seconds: 60,
            jitter_seconds: 5,
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
    /// 是否执行来源扫描；旧 `enabled` 未设置时默认开启，保证告警台可用。
    pub const fn scan_enabled(&self) -> bool {
        match (self.scan_enabled, self.enabled) {
            (Some(value), _) => value,
            (None, Some(value)) => value,
            (None, None) => true,
        }
    }

    /// 是否创建 MoogleMail 运维通知；旧 `enabled` 未设置时默认关闭。
    pub const fn notify_enabled(&self) -> bool {
        match (self.notify_enabled, self.enabled) {
            (Some(value), _) => value,
            (None, Some(value)) => value,
            (None, None) => false,
        }
    }

    /// 是否启用独立 webhook 通道。
    pub const fn webhook_enabled(&self) -> bool {
        self.webhook_enabled
    }

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

#[cfg(test)]
mod tests {
    use super::OperationsAlertWorkerConfig;

    #[test]
    fn defaults_run_scan_without_sending_notifications() {
        let config = OperationsAlertWorkerConfig::default();
        assert!(config.scan_enabled());
        assert!(!config.notify_enabled());
    }

    #[test]
    fn legacy_enabled_alias_still_sets_both_capabilities() {
        let config: OperationsAlertWorkerConfig =
            toml::from_str("enabled = true").expect("旧字段必须继续可解析");
        assert!(config.scan_enabled());
        assert!(config.notify_enabled());
    }

    #[test]
    fn split_fields_override_without_touching_the_other_capability() {
        let scan_only: OperationsAlertWorkerConfig =
            toml::from_str("scan_enabled = true\nnotify_enabled = false")
                .expect("拆分字段必须可解析");
        assert!(scan_only.scan_enabled());
        assert!(!scan_only.notify_enabled());
        let notify: OperationsAlertWorkerConfig =
            toml::from_str("scan_enabled = true\nnotify_enabled = true")
                .expect("拆分字段必须可解析");
        assert!(notify.notify_enabled());
    }
}
