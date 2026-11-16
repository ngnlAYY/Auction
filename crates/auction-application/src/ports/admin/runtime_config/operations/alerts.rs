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
    /// 长期未领取阈值秒数，范围 1–2592000；默认 72 小时，默认值省略以保持旧文档指纹。
    #[serde(skip_serializing_if = "is_default_collection_overdue_age")]
    pub fulfillment_collection_overdue_age_seconds: u64,
    pub recovery_overdue_count_threshold: i64,
    pub recovery_overdue_age_seconds: u64,
    /// 通知使用的游戏世界，只允许 main 或 isekai。
    pub world_id: String,
    /// 通知收件人，启用并接管告警时必填。
    pub recipient: String,
}

/// 长期未领取阈值的默认值；`skip_serializing_if` 判定与 `Default` 共用，避免两处漂移。
const DEFAULT_COLLECTION_OVERDUE_AGE_SECONDS: u64 = 72 * 3_600;

fn is_default_collection_overdue_age(value: &u64) -> bool {
    *value == DEFAULT_COLLECTION_OVERDUE_AGE_SECONDS
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
            fulfillment_collection_overdue_age_seconds: DEFAULT_COLLECTION_OVERDUE_AGE_SECONDS,
            recovery_overdue_count_threshold: 1,
            recovery_overdue_age_seconds: 900,
            world_id: "main".to_owned(),
            recipient: String::new(),
        }
    }
}

impl AdminRuntimeAlertsConfig {
    /// 物理文档按默认值省略的字段（JSON 键）及其默认值；Admin 浏览器投影据此补齐完整闭集。
    pub fn omitted_defaults() -> serde_json::Map<String, serde_json::Value> {
        let defaults = Self::default();
        let mut omitted = serde_json::Map::new();
        omitted.insert(
            "fulfillment_collection_overdue_age_seconds".to_owned(),
            defaults.fulfillment_collection_overdue_age_seconds.into(),
        );
        omitted
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
                self.fulfillment_collection_overdue_age_seconds,
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
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::AdminRuntimeAlertsConfig;

    fn serialized_keys(config: &AdminRuntimeAlertsConfig) -> BTreeSet<String> {
        match serde_json::to_value(config).expect("告警配置必须可序列化") {
            Value::Object(object) => object.into_iter().map(|(key, _)| key).collect(),
            other => panic!("告警配置必须序列化为对象: {other}"),
        }
    }

    /// 新阈值默认省略：旧文档与显式写出默认值的文档规范化字节相同，保持 runtime 指纹；
    /// 省略清单恰好是这一键，补齐后解析回默认值。
    #[test]
    fn collection_overdue_default_is_omitted_and_restored() {
        let old: AdminRuntimeAlertsConfig =
            serde_json::from_str("{\"managed\":true}").expect("旧告警文档");
        assert_eq!(old.fulfillment_collection_overdue_age_seconds, 259_200);
        let explicit: AdminRuntimeAlertsConfig = serde_json::from_str(
            "{\"managed\":true,\"fulfillment_collection_overdue_age_seconds\":259200}",
        )
        .expect("显式默认值文档");
        assert_eq!(
            serde_json::to_vec(&old).expect("旧文档规范化"),
            serde_json::to_vec(&explicit).expect("显式文档规范化")
        );
        let changed = AdminRuntimeAlertsConfig {
            fulfillment_collection_overdue_age_seconds: 86_400,
            ..AdminRuntimeAlertsConfig::default()
        };
        let omitted = AdminRuntimeAlertsConfig::omitted_defaults();
        assert_eq!(
            omitted.keys().cloned().collect::<BTreeSet<_>>(),
            serialized_keys(&changed)
                .difference(&serialized_keys(&AdminRuntimeAlertsConfig::default()))
                .cloned()
                .collect::<BTreeSet<_>>()
        );
        let mut merged = serde_json::to_value(AdminRuntimeAlertsConfig::default())
            .expect("默认配置必须可序列化");
        merged.as_object_mut().expect("默认配置必须是对象").extend(omitted);
        let parsed: AdminRuntimeAlertsConfig =
            serde_json::from_value(merged).expect("补齐后的默认文档必须可解析");
        assert_eq!(parsed, AdminRuntimeAlertsConfig::default());
    }

    /// 运行期阈值复用告警策略的 1 秒至 30 天边界。
    #[test]
    fn collection_overdue_threshold_is_validated_with_the_policy() {
        for (seconds, valid) in [(0, false), (1, true), (2_592_000, true), (2_592_001, false)] {
            let config = AdminRuntimeAlertsConfig {
                fulfillment_collection_overdue_age_seconds: seconds,
                ..AdminRuntimeAlertsConfig::default()
            };
            assert_eq!(config.validate().is_ok(), valid, "{seconds}");
        }
    }
}
