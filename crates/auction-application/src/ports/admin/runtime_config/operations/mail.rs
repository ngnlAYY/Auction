//! 邮件轮询与处理预算；邮箱身份、远程地址、凭据和全局启用由部署配置拥有。

use serde::{Deserialize, Deserializer, Serialize};

use super::require;
use crate::ports::admin::AdminRuntimeConfigError;
use crate::ports::delivery::DeliverySendIntervals;
use crate::ports::mail::MailSendClasses;

/// Admin 启动时从真实 Worker 部署文件取得的非敏感跨组预算快照。
/// 不进入 runtime 文档、命令摘要或 DTO，也不以 Admin 自身的 Worker 默认值补齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminRuntimeMailDeliveryBudget {
    pub mail_enabled: bool,
    pub mail_fetch_timeout_seconds: u64,
    pub delivery_request_timeout_seconds: u64,
}

impl AdminRuntimeMailDeliveryBudget {
    /// 保存候选与 Worker 启动共用的跨组约束；各预算自身范围仍由原 owner 校验。
    pub const fn is_valid(self) -> bool {
        !self.mail_enabled
            || self.delivery_request_timeout_seconds
                > self.mail_fetch_timeout_seconds.saturating_mul(2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeMailConfig {
    /// 是否接管邮件运行参数；默认关闭，不改变既有 Worker 行为。
    pub managed: bool,
    /// 同类邮件开始发送的最小间距；全零省略。Worker 热读取成功后在下一个 delivery 领取生效。
    #[serde(skip_serializing_if = "DeliverySendIntervals::is_zero")]
    pub send_intervals: DeliverySendIntervals,
    /// 标准五字段 cron、旧版描述符或 @every；完整解析由配置适配器校验。
    pub schedule: String,
    /// 单次响应最大字节数，范围 1–8 MiB。
    pub maximum_response_bytes: usize,
    /// 连续请求最小间隔毫秒数，范围 1–60000。
    pub minimum_request_interval_millis: u64,
    /// 单次 HTTP 请求超时秒数，范围 1–300。
    pub request_timeout_seconds: u64,
    /// 每轮最多读取邮件数，范围 1–1000。
    pub batch_size: u16,
    /// 每轮最多读取页数，范围 1–100。
    pub maximum_pages: usize,
    /// 计划触发后随机抖动上限秒数，范围 0–86400。
    pub jitter_seconds: u64,
    /// 邮箱 claim 租约秒数，范围 1–86400。
    pub lease_duration_seconds: u64,
    /// 续租周期秒数，必须大于零且小于租约。
    pub renew_interval_seconds: u64,
    /// 每轮远程读取内联尝试次数，范围 1–10。
    pub fetch_attempts: u16,
    /// 单次读取总超时秒数，范围 1–300，且大于 HTTP 请求超时。
    pub fetch_timeout_seconds: u64,
    /// 单封邮件业务处理内联尝试次数，范围 1–10。
    pub process_attempts: u16,
    /// 单次邮件处理总超时秒数，范围 1–300，且大于 HTTP 请求超时。
    pub item_timeout_seconds: u64,
    /// 单次邮箱 claim 的总处理预算秒数；耗尽时只提交已完成前缀并推进到其邮件 ID。
    #[serde(skip_serializing_if = "is_default_batch_timeout")]
    pub batch_timeout_seconds: u64,
    /// 指数退避基础秒数，范围 1–300。
    pub retry_base_seconds: u64,
    /// 指数退避上限秒数，范围 1–300，且不小于基础退避。
    pub retry_maximum_seconds: u64,
    /// 是否接受登录 key 自助轮换主题；默认 true，省略以保持旧文档指纹。
    #[serde(skip_serializing_if = "is_true")]
    pub rekey_enabled: bool,
    /// 同一账号注册/轮换回信窗口秒数，范围 60–604800；默认值省略。
    #[serde(skip_serializing_if = "is_default_reply_window")]
    pub account_reply_window_seconds: u64,
    /// 说明信窗口秒数，范围 60–604800；默认值省略。
    #[serde(skip_serializing_if = "is_default_explanation_window")]
    pub explanation_mail_window_seconds: u64,
    /// 说明信窗口内最大封数，范围 0–100；0 表示不限；默认值省略。
    #[serde(skip_serializing_if = "is_default_explanation_limit")]
    pub explanation_mail_limit: u32,
    /// 六类出站通知的发送开关；默认全部开启，默认值省略以保持旧文档指纹。
    /// 关闭只抑制通知入队，不改变已提交的命令、履约义务或审计事实。
    #[serde(skip_serializing_if = "is_default_send_classes")]
    pub send_classes: MailSendClasses,
}

// 按默认值省略的标量字段的默认值；`skip_serializing_if` 判定与 `Default` 共用，避免两处漂移。
const DEFAULT_BATCH_TIMEOUT_SECONDS: u64 = 600;
const DEFAULT_REPLY_WINDOW_SECONDS: u64 = 86_400;
const DEFAULT_EXPLANATION_WINDOW_SECONDS: u64 = 86_400;
const DEFAULT_EXPLANATION_LIMIT: u32 = 10;

fn is_default_send_classes(value: &MailSendClasses) -> bool {
    *value == MailSendClasses::default()
}

fn is_default_batch_timeout(value: &u64) -> bool {
    *value == DEFAULT_BATCH_TIMEOUT_SECONDS
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_default_reply_window(value: &u64) -> bool {
    *value == DEFAULT_REPLY_WINDOW_SECONDS
}

fn is_default_explanation_window(value: &u64) -> bool {
    *value == DEFAULT_EXPLANATION_WINDOW_SECONDS
}

fn is_default_explanation_limit(value: &u32) -> bool {
    *value == DEFAULT_EXPLANATION_LIMIT
}

impl Default for AdminRuntimeMailConfig {
    fn default() -> Self {
        Self {
            managed: false,
            send_intervals: DeliverySendIntervals::default(),
            schedule: "*/5 * * * *".to_owned(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval_millis: 1000,
            request_timeout_seconds: 15,
            batch_size: 100,
            maximum_pages: 20,
            jitter_seconds: 5,
            lease_duration_seconds: 60,
            renew_interval_seconds: 10,
            fetch_attempts: 3,
            fetch_timeout_seconds: 30,
            process_attempts: 3,
            item_timeout_seconds: 30,
            batch_timeout_seconds: DEFAULT_BATCH_TIMEOUT_SECONDS,
            retry_base_seconds: 1,
            retry_maximum_seconds: 30,
            rekey_enabled: true,
            account_reply_window_seconds: DEFAULT_REPLY_WINDOW_SECONDS,
            explanation_mail_window_seconds: DEFAULT_EXPLANATION_WINDOW_SECONDS,
            explanation_mail_limit: DEFAULT_EXPLANATION_LIMIT,
            send_classes: MailSendClasses::default(),
        }
    }
}

impl AdminRuntimeMailConfig {
    /// 物理文档按默认值省略的字段（JSON 键）及其默认值。Admin 浏览器投影据此补齐完整闭集，
    /// 默认值只有这一个来源；`omitted_defaults_are_exactly_the_fields_skipped_at_default`
    /// 保证它与各字段的 `skip_serializing_if` 一一对应。
    pub fn omitted_defaults() -> serde_json::Result<serde_json::Map<String, serde_json::Value>> {
        let defaults = Self::default();
        let mut omitted = serde_json::Map::new();
        omitted.insert("send_intervals".to_owned(), serde_json::to_value(defaults.send_intervals)?);
        omitted.insert("batch_timeout_seconds".to_owned(), defaults.batch_timeout_seconds.into());
        omitted.insert("rekey_enabled".to_owned(), defaults.rekey_enabled.into());
        omitted.insert(
            "account_reply_window_seconds".to_owned(),
            defaults.account_reply_window_seconds.into(),
        );
        omitted.insert(
            "explanation_mail_window_seconds".to_owned(),
            defaults.explanation_mail_window_seconds.into(),
        );
        omitted.insert("explanation_mail_limit".to_owned(), defaults.explanation_mail_limit.into());
        omitted.insert("send_classes".to_owned(), serde_json::to_value(defaults.send_classes)?);
        Ok(omitted)
    }

    pub(super) fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        require(self.send_intervals.is_valid(), "mail.send_intervals")?;
        require(auction_domain::valid_text(&self.schedule, 256), "mail.schedule")?;
        require(
            (1..=8 * 1024 * 1024).contains(&self.maximum_response_bytes),
            "mail.maximum_response_bytes",
        )?;
        require(
            (1..=60_000).contains(&self.minimum_request_interval_millis),
            "mail.minimum_request_interval_millis",
        )?;
        require((1..=300).contains(&self.request_timeout_seconds), "mail.request_timeout_seconds")?;
        require((1..=1000).contains(&self.batch_size), "mail.batch_size")?;
        require((1..=100).contains(&self.maximum_pages), "mail.maximum_pages")?;
        require(self.jitter_seconds <= 86_400, "mail.jitter_seconds")?;
        require(
            (1..=86_400).contains(&self.lease_duration_seconds),
            "mail.lease_duration_seconds",
        )?;
        require(
            self.renew_interval_seconds > 0
                && self.renew_interval_seconds < self.lease_duration_seconds,
            "mail.renew_interval_seconds",
        )?;
        require((1..=10).contains(&self.fetch_attempts), "mail.fetch_attempts")?;
        require(
            (1..=300).contains(&self.fetch_timeout_seconds)
                && self.fetch_timeout_seconds > self.request_timeout_seconds,
            "mail.fetch_timeout_seconds",
        )?;
        require((1..=10).contains(&self.process_attempts), "mail.process_attempts")?;
        require(
            (1..=300).contains(&self.item_timeout_seconds)
                && self.item_timeout_seconds > self.request_timeout_seconds,
            "mail.item_timeout_seconds",
        )?;
        require(
            (1..=86_400).contains(&self.batch_timeout_seconds)
                && self.batch_timeout_seconds
                    >= self.fetch_timeout_seconds.saturating_add(self.item_timeout_seconds),
            "mail.batch_timeout_seconds",
        )?;
        require(
            self.retry_base_seconds > 0
                && self.retry_base_seconds <= self.retry_maximum_seconds
                && self.retry_maximum_seconds <= 300,
            "mail.retry_maximum_seconds",
        )?;
        require(
            (60..=604_800).contains(&self.account_reply_window_seconds),
            "mail.account_reply_window_seconds",
        )?;
        require(
            (60..=604_800).contains(&self.explanation_mail_window_seconds),
            "mail.explanation_mail_window_seconds",
        )?;
        require(self.explanation_mail_limit <= 100, "mail.explanation_mail_limit")
    }
}

/// 省略区块保持旧指纹，显式 null 则拒绝，避免被误解为撤销接管。
pub(super) fn deserialize_optional<'de, D>(
    deserializer: D,
) -> Result<Option<AdminRuntimeMailConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    AdminRuntimeMailConfig::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::AdminRuntimeMailConfig;
    use crate::ports::delivery::DeliverySendIntervals;
    use crate::ports::mail::MailSendClasses;

    /// 每个字段都不等于默认值。结构体字面量不写 `..`：新增字段时这里编译失败，迫使作者同时判断
    /// 新字段是否按默认值省略、是否要登记进 `omitted_defaults`。
    fn every_field_changed() -> AdminRuntimeMailConfig {
        AdminRuntimeMailConfig {
            managed: true,
            send_intervals: DeliverySendIntervals {
                outbid_seconds: 30,
                ..DeliverySendIntervals::default()
            },
            schedule: "@every 10m".to_owned(),
            maximum_response_bytes: 1024,
            minimum_request_interval_millis: 500,
            request_timeout_seconds: 20,
            batch_size: 50,
            maximum_pages: 10,
            jitter_seconds: 0,
            lease_duration_seconds: 120,
            renew_interval_seconds: 20,
            fetch_attempts: 2,
            fetch_timeout_seconds: 40,
            process_attempts: 2,
            item_timeout_seconds: 40,
            batch_timeout_seconds: 900,
            retry_base_seconds: 2,
            retry_maximum_seconds: 60,
            rekey_enabled: false,
            account_reply_window_seconds: 3_600,
            explanation_mail_window_seconds: 7_200,
            explanation_mail_limit: 0,
            send_classes: MailSendClasses { player_summary: false, ..MailSendClasses::default() },
        }
    }

    fn serialized_keys(config: &AdminRuntimeMailConfig) -> BTreeSet<String> {
        match serde_json::to_value(config).expect("邮件配置必须可序列化") {
            Value::Object(object) => object.into_iter().map(|(key, _)| key).collect(),
            other => panic!("邮件配置必须序列化为对象: {other}"),
        }
    }

    /// 省略清单恰好是“非默认时出现、默认时省略”的键，且补齐后解析回 `Default`。
    #[test]
    fn omitted_defaults_are_exactly_the_fields_skipped_at_default() {
        let all = serialized_keys(&every_field_changed());
        let present = serialized_keys(&AdminRuntimeMailConfig::default());
        let omitted = AdminRuntimeMailConfig::omitted_defaults().expect("默认值必须可序列化");
        assert_eq!(
            omitted.keys().cloned().collect::<BTreeSet<_>>(),
            all.difference(&present).cloned().collect::<BTreeSet<_>>()
        );
        let mut merged =
            serde_json::to_value(AdminRuntimeMailConfig::default()).expect("默认配置必须可序列化");
        merged.as_object_mut().expect("默认配置必须是对象").extend(omitted);
        let parsed: AdminRuntimeMailConfig =
            serde_json::from_value(merged).expect("补齐后的默认文档必须可解析");
        assert_eq!(parsed, AdminRuntimeMailConfig::default());
    }

    #[test]
    fn missing_reply_fields_use_defaults_in_partial_documents() {
        let config: AdminRuntimeMailConfig =
            serde_json::from_str("{\"managed\":true}").expect("partial mail runtime document");
        assert!(config.rekey_enabled);
        assert_eq!(config.account_reply_window_seconds, 86_400);
        assert_eq!(config.explanation_mail_window_seconds, 86_400);
        assert_eq!(config.explanation_mail_limit, 10);
    }

    #[test]
    fn default_reply_fields_do_not_change_canonical_json() {
        let old: AdminRuntimeMailConfig =
            serde_json::from_str("{\"managed\":true}").expect("old mail runtime document");
        let explicit: AdminRuntimeMailConfig = serde_json::from_str(
            "{\"managed\":true,\"rekey_enabled\":true,\"account_reply_window_seconds\":86400,\"explanation_mail_window_seconds\":86400,\"explanation_mail_limit\":10}",
        )
        .expect("explicit default mail runtime document");
        assert_eq!(
            serde_json::to_vec(&old).expect("old canonical JSON"),
            serde_json::to_vec(&explicit).expect("new canonical JSON")
        );
        let encoded: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&old).expect("canonical JSON"))
                .expect("canonical JSON value");
        let mail = encoded.as_object().expect("mail object");
        assert!(!mail.contains_key("rekey_enabled"));
        assert!(!mail.contains_key("account_reply_window_seconds"));
        assert!(!mail.contains_key("explanation_mail_window_seconds"));
        assert!(!mail.contains_key("explanation_mail_limit"));
    }

    #[test]
    fn old_mail_documents_keep_their_fingerprint_across_reserialization() {
        let raw = "{\"operations\":{\"mail\":{\"managed\":true}}}";
        let document: crate::ports::admin::runtime_config::AdminRuntimeDocument =
            serde_json::from_str(raw).expect("old runtime document");
        let fingerprint = document.fingerprint();
        let encoded = serde_json::to_string(&document).expect("canonical runtime document");
        let reparsed: crate::ports::admin::runtime_config::AdminRuntimeDocument =
            serde_json::from_str(&encoded).expect("reparsed runtime document");
        assert_eq!(reparsed.fingerprint(), fingerprint);
    }
}
