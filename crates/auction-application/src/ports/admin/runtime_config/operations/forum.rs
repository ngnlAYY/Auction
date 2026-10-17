//! 论坛同步候选及完整请求预算；远程地址、凭据和测试板块继续由部署配置拥有。

use serde::{Deserialize, Serialize};

use super::{AdminRuntimeConfigError, require};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeForumConfig {
    /// 是否由管理页覆盖论坛同步参数，默认关闭。
    pub managed: bool,
    /// 是否启用主题发布和回帖同步。
    pub enabled: bool,
    /// 每轮论坛任务批量上限。
    pub batch_size: u16,
    pub poll_interval_seconds: u64,
    pub jitter_seconds: u64,
    pub maximum_response_bytes: usize,
    /// 单次主题扫描最多页数。
    pub maximum_topic_pages: u16,
    pub minimum_request_interval_millis: u64,
    pub request_timeout_seconds: u64,
    pub item_timeout_seconds: u64,
    /// 单个主题任务 claim 租约秒数。
    pub lease_duration_seconds: u64,
}

impl Default for AdminRuntimeForumConfig {
    fn default() -> Self {
        Self {
            managed: false,
            enabled: false,
            batch_size: 100,
            poll_interval_seconds: 300,
            jitter_seconds: 5,
            maximum_response_bytes: 2 * 1024 * 1024,
            maximum_topic_pages: 10,
            minimum_request_interval_millis: 750,
            request_timeout_seconds: 30,
            item_timeout_seconds: 240,
            lease_duration_seconds: 300,
        }
    }
}

impl AdminRuntimeForumConfig {
    pub(super) fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        require((1..=100).contains(&self.batch_size), "forum.batch_size")?;
        require((1..=86_400).contains(&self.poll_interval_seconds), "forum.poll_interval_seconds")?;
        require(self.jitter_seconds <= 86_400, "forum.jitter_seconds")?;
        require(
            (1..=8 * 1024 * 1024).contains(&self.maximum_response_bytes),
            "forum.maximum_response_bytes",
        )?;
        require((1..=20).contains(&self.maximum_topic_pages), "forum.maximum_topic_pages")?;
        require(
            (1..=60_000).contains(&self.minimum_request_interval_millis),
            "forum.minimum_request_interval_millis",
        )?;
        require(
            (1..=300).contains(&self.request_timeout_seconds),
            "forum.request_timeout_seconds",
        )?;
        require((1..=3_600).contains(&self.item_timeout_seconds), "forum.item_timeout_seconds")?;
        require(
            (1..=86_400).contains(&self.lease_duration_seconds)
                && self.item_timeout_seconds < self.lease_duration_seconds,
            "forum.lease_duration_seconds",
        )?;
        // 单目录页的最低预算：四路主题分页读取，再为目录/日志各预留 GET + POST，最后留一秒收口。
        // 多目录页及正文校验会增加请求；部署须相应提高 item/lease，总时限仍保护未知写入边界。
        // 上方先限制各输入，以下整数运算不会溢出。
        let pages = u64::from(self.maximum_topic_pages);
        let waves = pages.div_ceil(4) + 4;
        let budget_millis = self.request_timeout_seconds * waves * 1_000
            + self.minimum_request_interval_millis * (pages + 4)
            + 1_000;
        require(self.item_timeout_seconds * 1_000 >= budget_millis, "forum.item_timeout_seconds")
    }
}
