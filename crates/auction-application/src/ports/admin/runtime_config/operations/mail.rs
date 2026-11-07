//! 邮件轮询与处理预算；邮箱身份、远程地址、凭据和全局启用由部署配置拥有。

use serde::{Deserialize, Deserializer, Serialize};

use crate::ports::delivery::DeliverySendIntervals;

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
}

fn is_default_batch_timeout(value: &u64) -> bool {
    *value == 600
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
        )
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
