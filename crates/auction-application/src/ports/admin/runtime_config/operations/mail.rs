//! 邮件轮询与处理预算；邮箱身份、远程地址、凭据和全局启用由部署配置拥有。

use serde::{Deserialize, Deserializer, Serialize};

use crate::ports::delivery::DeliverySendIntervals;
use crate::ports::mail::MailSendClasses;

use super::require;
use crate::ports::admin::AdminRuntimeConfigError;

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

fn is_default_send_classes(value: &MailSendClasses) -> bool {
    *value == MailSendClasses::default()
}

fn is_default_batch_timeout(value: &u64) -> bool {
    *value == 600
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_default_reply_window(value: &u64) -> bool {
    *value == 86_400
}

fn is_default_explanation_window(value: &u64) -> bool {
    *value == 86_400
}

fn is_default_explanation_limit(value: &u32) -> bool {
    *value == 10
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
            batch_timeout_seconds: 600,
            retry_base_seconds: 1,
            retry_maximum_seconds: 30,
            rekey_enabled: true,
            account_reply_window_seconds: 86_400,
            explanation_mail_window_seconds: 86_400,
            explanation_mail_limit: 10,
            send_classes: MailSendClasses::default(),
        }
    }
}

impl AdminRuntimeMailConfig {
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
    use super::AdminRuntimeMailConfig;

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
