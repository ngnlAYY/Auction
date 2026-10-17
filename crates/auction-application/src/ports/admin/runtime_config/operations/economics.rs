//! 后续新场次的经济参数候选；复用领域校验，不建立第二套竞价状态机。

use auction_domain::MAX_BID_AMOUNT;
use chrono::Duration;
use serde::{Deserialize, Serialize};

use crate::AuctionPolicyTemplate;

use super::{AdminRuntimeConfigError, invalid, require};

/// 由管理页选择接管的新拍卖默认策略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminRuntimeEconomicsConfig {
    /// 是否覆盖公共 TOML 的经济模板；默认关闭。
    pub managed: bool,
    /// 默认起始价，必须按 1K 网格对齐且不超过 1B。
    pub start_price: i64,
    /// 加价百万分比；零值继续使用既有固定最低增量。
    pub bid_increment_ppm: i64,
    /// 软截止触发窗口秒数。
    pub soft_close_window_seconds: i64,
    /// 每次软截止延长秒数。
    pub soft_close_extension_seconds: i64,
    /// 取消截止窗口秒数。
    pub cancel_cutoff_seconds: i64,
    /// 是否允许代理竞价。
    pub proxy_enabled: bool,
    /// 默认代理模式，只允许 fast 或 slow。
    pub proxy_default_mode: String,
    /// 慢速代理间隔秒数。
    pub proxy_slow_interval_seconds: i64,
    /// 被超价使用次数上限；零表示不限制。
    pub proxy_max_outbid_uses: u32,
    /// 最终阶段提前量秒数。
    pub final_phase_lead_seconds: i64,
}

impl Default for AdminRuntimeEconomicsConfig {
    fn default() -> Self {
        Self {
            managed: false,
            start_price: 1_000,
            bid_increment_ppm: 50_000,
            soft_close_window_seconds: 900,
            soft_close_extension_seconds: 900,
            cancel_cutoff_seconds: 86_400,
            proxy_enabled: true,
            proxy_default_mode: "fast".to_owned(),
            proxy_slow_interval_seconds: 600,
            proxy_max_outbid_uses: 3,
            final_phase_lead_seconds: 900,
        }
    }
}

impl AdminRuntimeEconomicsConfig {
    pub(super) fn validate(&self) -> Result<(), AdminRuntimeConfigError> {
        require((1..=MAX_BID_AMOUNT).contains(&self.start_price), "economics.start_price")?;
        // 先限制秒数，再构造 chrono Duration，避免任意 i64 输入在转换时溢出。
        for (seconds, minimum, field) in [
            (self.soft_close_window_seconds, 0, "economics.soft_close_window_seconds"),
            (self.soft_close_extension_seconds, 1, "economics.soft_close_extension_seconds"),
            (self.cancel_cutoff_seconds, 0, "economics.cancel_cutoff_seconds"),
            (self.proxy_slow_interval_seconds, 1, "economics.proxy_slow_interval_seconds"),
            (self.final_phase_lead_seconds, 1, "economics.final_phase_lead_seconds"),
        ] {
            require((minimum..=604_800).contains(&seconds), field)?;
        }
        AuctionPolicyTemplate {
            start_price: self.start_price,
            bid_increment_ppm: self.bid_increment_ppm,
            soft_close_window: Duration::seconds(self.soft_close_window_seconds),
            soft_close_extension: Duration::seconds(self.soft_close_extension_seconds),
            cancel_cutoff: Duration::seconds(self.cancel_cutoff_seconds),
            proxy_enabled: self.proxy_enabled,
            proxy_default_mode: self.proxy_default_mode.clone(),
            proxy_slow_interval: Duration::seconds(self.proxy_slow_interval_seconds),
            proxy_max_outbid_uses: self.proxy_max_outbid_uses,
            final_phase_lead: Duration::seconds(self.final_phase_lead_seconds),
            ..AuctionPolicyTemplate::default()
        }
        .validate()
        .map_err(|_| invalid("economics"))
    }
}
