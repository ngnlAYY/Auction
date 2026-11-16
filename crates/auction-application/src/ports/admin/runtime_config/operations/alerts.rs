//! 运维告警候选，复用应用告警策略的批次、租约、阈值和收件人校验。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{invalid, require};
use crate::ports::admin::AdminRuntimeConfigError;
use crate::ports::operations::{OperationsAlertPolicy, OperationsAlertTarget};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeAlertsConfig {
    /// 是否由管理页覆盖运维告警参数，默认关闭。
    pub managed: bool,
    /// 是否启用告警扫描与告警台；主动 MoogleMail 通知由 Worker TOML 独立控制。
    pub enabled: bool,
    pub scan_interval_seconds: u64,
    /// 不大于扫描间隔的随机抖动秒数。
    pub jitter_seconds: u64,
    pub source_timeout_seconds: u64,
    pub batch_window_seconds: u64,
    pub cooldown_seconds: u64,
    /// 告警批次租约秒数，必须覆盖四倍来源读取超时。
    pub claim_lease_seconds: u64,
    /// 失败告警的持久化重试间隔秒数。
    pub retry_delay_seconds: u64,
    /// 每轮最多处理的告警信号数。
    pub max_signals_per_scan: usize,
    /// 单个通知批次最多信号数。
    pub max_batch_items: usize,
    pub max_body_bytes: usize,
    pub delivery_pending_count_threshold: i64,
    pub delivery_pending_age_seconds: u64,
    pub delivery_manual_reconcile_count_threshold: i64,
    pub fulfillment_backlog_count_threshold: i64,
    pub fulfillment_backlog_age_seconds: u64,
    /// 长期未领取阈值秒数，范围 1–2592000；省略即默认 72 小时，生效值见
    /// `collection_overdue_age_seconds`。物理文档不写默认值，保持旧文档指纹、回退的旧程序仍能
    /// 加载；写入请求省略该键表示保持当前值（升级前的页面不认识它），由更新用例在指纹 fence
    /// 内按当前文件补回。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fulfillment_collection_overdue_age_seconds: Option<u64>,
    pub recovery_overdue_count_threshold: i64,
    pub recovery_overdue_age_seconds: u64,
    /// 通知使用的游戏世界，只允许 main 或 isekai。
    pub world_id: String,
    /// 通知收件人，启用并接管告警时必填。
    pub recipient: String,
}

/// 长期未领取阈值的默认值；省略与显式写出默认值等价，保存时按省略处理。
const DEFAULT_COLLECTION_OVERDUE_AGE_SECONDS: u64 = 72 * 3_600;

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
            fulfillment_collection_overdue_age_seconds: None,
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_seconds: 900,
            world_id: "main".to_owned(),
            recipient: String::new(),
        }
    }
}

impl AdminRuntimeAlertsConfig {
    /// 生效的长期未领取阈值秒数；省略时为默认 72 小时。
    pub fn collection_overdue_age_seconds(&self) -> u64 {
        self.fulfillment_collection_overdue_age_seconds
            .unwrap_or(DEFAULT_COLLECTION_OVERDUE_AGE_SECONDS)
    }

    /// 写入请求的兼容合并：省略的长期未领取阈值沿用 `current` 的值；显式写出的默认值按省略
    /// 保存。结果只由请求与当前文件决定，可重复执行。
    pub(crate) fn resolve_update(&mut self, current: Option<&Self>) {
        if self.fulfillment_collection_overdue_age_seconds.is_none() {
            self.fulfillment_collection_overdue_age_seconds =
                current.and_then(|current| current.fulfillment_collection_overdue_age_seconds);
        }
        if self.fulfillment_collection_overdue_age_seconds
            == Some(DEFAULT_COLLECTION_OVERDUE_AGE_SECONDS)
        {
            self.fulfillment_collection_overdue_age_seconds = None;
        }
    }

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
            fulfillment_collection_overdue_age_threshold: Duration::from_secs(
                self.collection_overdue_age_seconds(),
            ),
            recovery_overdue_count_threshold: self.recovery_overdue_count_threshold,
            recovery_overdue_age_threshold: Duration::from_secs(self.recovery_overdue_age_seconds),
        }
        .validate()
        .map_err(|_| invalid("alerts"))?;
        require(matches!(self.world_id.as_str(), "main" | "isekai"), "alerts.world_id")?;
        if !self.recipient.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::AdminRuntimeAlertsConfig;

    fn with_threshold(seconds: Option<u64>) -> AdminRuntimeAlertsConfig {
        AdminRuntimeAlertsConfig {
            fulfillment_collection_overdue_age_seconds: seconds,
            ..AdminRuntimeAlertsConfig::default()
        }
    }

    /// 新阈值省略即默认 72 小时，且不写入物理文档：旧文档的规范化字节不变，保持 runtime 指纹。
    #[test]
    fn collection_overdue_threshold_is_omitted_by_default() {
        let old: AdminRuntimeAlertsConfig =
            serde_json::from_str("{\"managed\":true}").expect("旧告警文档");
        assert_eq!(old.fulfillment_collection_overdue_age_seconds, None);
        assert_eq!(old.collection_overdue_age_seconds(), 259_200);
        let encoded = serde_json::to_string(&old).expect("告警配置必须可序列化");
        assert!(!encoded.contains("fulfillment_collection_overdue_age_seconds"), "{encoded}");
        let custom = with_threshold(Some(86_400));
        assert_eq!(custom.collection_overdue_age_seconds(), 86_400);
        let encoded = serde_json::to_value(&custom).expect("告警配置必须可序列化");
        assert_eq!(encoded["fulfillment_collection_overdue_age_seconds"], 86_400);
    }

    /// 写入合并：请求省略时沿用当前值（升级前的页面保存不得重置它），显式值优先；显式默认值
    /// 按省略保存；结果可重复执行。
    #[test]
    fn update_keeps_the_current_threshold_when_the_request_omits_it() {
        let current = with_threshold(Some(86_400));
        for (request, current, expected) in [
            (None, Some(&current), Some(86_400)),
            (None, None, None),
            (None, Some(&with_threshold(None)), None),
            (Some(3_600), Some(&current), Some(3_600)),
            (Some(259_200), Some(&current), None),
            (None, Some(&with_threshold(Some(259_200))), None),
        ] {
            let mut resolved = with_threshold(request);
            resolved.resolve_update(current);
            assert_eq!(
                resolved.fulfillment_collection_overdue_age_seconds, expected,
                "{request:?} / {current:?}"
            );
            let mut again = resolved.clone();
            again.resolve_update(Some(&resolved));
            assert_eq!(again, resolved, "合并可重复执行");
        }
    }

    /// 运行期阈值复用告警策略的 1 秒至 30 天边界。
    #[test]
    fn collection_overdue_threshold_is_validated_with_the_policy() {
        for (seconds, valid) in [(0, false), (1, true), (2_592_000, true), (2_592_001, false)] {
            assert_eq!(with_threshold(Some(seconds)).validate().is_ok(), valid, "{seconds}");
        }
        assert!(with_threshold(None).validate().is_ok());
    }
}
