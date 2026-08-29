//! 拍卖创建时冻结的经济、结算和恢复策略配置。

use chrono::Duration;
use serde::Deserialize;

use auction_application::AuctionPolicyTemplate;
use auction_domain::{
    AdjustmentAnchor, RecoveryMode, RecoveryPolicy, SettlementMode, SettlementPolicy,
};

/// 拍卖创建时冻结的非敏感策略配置。
///
/// 该区块对应旧版共享配置中的 `auction.economics` 与 `auction.settlement`。所有字段都可
/// 省略；应用层会在创建事务中把已校验模板同时写入 JSON 快照和遗留规则列。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionConfig {
    /// 竞价经济和生命周期参数。
    pub economics: AuctionEconomicsConfig,
    /// 结算曲线和恢复策略参数。
    pub settlement: AuctionSettlementConfig,
    /// 每周轮次生成、preview/open 和 MoogleMail gate 的稳定计划。
    pub schedule: AuctionScheduleConfig,
}

/// 按旧版共享配置表达的本地周拍卖时间表。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionScheduleConfig {
    /// 是否启用 Worker 周计划；省略时关闭，避免开发数据库自动建轮次。
    pub enabled: bool,
    /// 计划使用的 IANA 时区。
    pub timezone: String,
    /// preview 的星期名称，例如 Friday。
    pub preview_day: String,
    /// preview 的本地时间，格式 HH:MM。
    pub preview_time: String,
    /// open 的星期名称，例如 Friday。
    pub open_day: String,
    /// open 的本地时间，格式 HH:MM。
    pub open_time: String,
    /// 名义结束的星期名称，例如 Saturday。
    pub nominal_end_day: String,
    /// 名义结束的本地时间，格式 HH:MM。
    pub nominal_end_time: String,
    /// 参与周计划的 world；省略时使用 main 和 isekai。
    pub worlds: Vec<String>,
}

impl Default for AuctionScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timezone: "Asia/Shanghai".to_owned(),
            preview_day: "Friday".to_owned(),
            preview_time: "10:00".to_owned(),
            open_day: "Friday".to_owned(),
            open_time: "12:00".to_owned(),
            nominal_end_day: "Saturday".to_owned(),
            nominal_end_time: "22:00".to_owned(),
            worlds: vec!["main".to_owned(), "isekai".to_owned()],
        }
    }
}

impl AuctionConfig {
    /// 将已通过主配置校验的 TOML 设置转换为应用层策略模板。
    pub fn policy(&self) -> AuctionPolicyTemplate {
        let settlement_mode = match self.settlement.mode.as_str() {
            "legacy_zero_v0" => SettlementMode::LegacyZeroV0,
            _ => SettlementMode::HoldCurveV1,
        };
        let recovery_mode = match self.settlement.recovery.mode.as_str() {
            "remind_v1" => RecoveryMode::RemindV1,
            _ => RecoveryMode::RemindV1,
        };
        AuctionPolicyTemplate {
            start_price: self.economics.start_price,
            bid_increment_ppm: self.economics.bid_increment_ppm,
            soft_close_window: Duration::seconds(self.economics.soft_close_window_seconds),
            soft_close_extension: Duration::seconds(self.economics.soft_close_extension_seconds),
            proxy_enabled: self.economics.proxy_enabled,
            proxy_default_mode: self.economics.proxy_default_mode.clone(),
            proxy_slow_interval: Duration::seconds(self.economics.proxy_slow_interval_seconds),
            proxy_max_outbid_uses: self.economics.proxy_max_outbid_uses,
            final_phase_lead: Duration::seconds(self.economics.final_phase_lead_seconds),
            cancel_cutoff: Duration::seconds(self.economics.cancel_cutoff_seconds),
            settlement: SettlementPolicy {
                version: self.settlement.version,
                mode: settlement_mode,
                compensation_pool_enabled: self.settlement.compensation_pool_enabled,
                isekai_carrier_credit: self.settlement.isekai_carrier_credit,
                anchors: self
                    .settlement
                    .anchors
                    .iter()
                    .map(|anchor| AdjustmentAnchor {
                        hold_ppm: anchor.hold_ppm,
                        adjustment_ppm: anchor.adjustment_ppm,
                    })
                    .collect(),
            },
            recovery: RecoveryPolicy {
                version: self.settlement.recovery.version,
                mode: recovery_mode,
                payment_timeout: Duration::seconds(
                    self.settlement.recovery.payment_timeout_seconds,
                ),
                reminder_limit: self.settlement.recovery.reminder_limit,
            },
        }
    }
}

/// 拍卖经济参数；创建拍卖时冻结到策略快照，并由竞价领域在运行时使用。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionEconomicsConfig {
    /// 默认起始价，必须为正且按 1000 信用网格对齐；省略时沿用旧版共享配置的 1000。
    pub start_price: i64,
    /// 经济策略的百万分比增量，范围 0-1000000；省略时为旧版共享配置的 50000。
    pub bid_increment_ppm: i64,
    /// 软截止窗口秒数，不能为负；省略时为 60 秒。
    pub soft_close_window_seconds: i64,
    /// 软截止延长秒数，必须大于 0；省略时为 60 秒。
    pub soft_close_extension_seconds: i64,
    /// 取消截止窗口秒数，不能为负；省略时为 86400 秒。
    pub cancel_cutoff_seconds: i64,
    /// 是否启用代理竞价；省略时启用。
    pub proxy_enabled: bool,
    /// 新代理授权默认模式，只能是 fast 或 slow；省略时为 fast。
    pub proxy_default_mode: String,
    /// 代理 slow 模式间隔秒数，必须大于 0；省略时为 600 秒。
    pub proxy_slow_interval_seconds: i64,
    /// 单个代理授权允许的 outbid 使用次数，省略时为 3；0 表示不限制。
    pub proxy_max_outbid_uses: u32,
    /// final phase 领先秒数，范围 1-604800；省略时为 60 秒。
    pub final_phase_lead_seconds: i64,
}

impl Default for AuctionEconomicsConfig {
    fn default() -> Self {
        Self {
            start_price: 1_000,
            bid_increment_ppm: 50_000,
            soft_close_window_seconds: 60,
            soft_close_extension_seconds: 60,
            cancel_cutoff_seconds: 86_400,
            proxy_enabled: true,
            proxy_default_mode: "fast".to_owned(),
            proxy_slow_interval_seconds: 600,
            proxy_max_outbid_uses: 3,
            final_phase_lead_seconds: 60,
        }
    }
}

/// 结算策略配置；锚点顺序和闭集模式由应用/领域校验。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionSettlementConfig {
    /// 结算策略版本；当前支持 0 或 1，省略时为 1。
    pub version: u32,
    /// 结算模式，只能是 hold_curve_v1 或 legacy_zero_v0；省略时为 hold_curve_v1。
    pub mode: String,
    /// 是否启用补偿池；省略时启用。
    pub compensation_pool_enabled: bool,
    /// isekai carrier credit 数量；当前策略要求为 1。
    pub isekai_carrier_credit: u32,
    /// 持有时长与调整比例的 PPM 锚点；省略时使用内置 hold curve。
    pub anchors: Vec<AuctionAdjustmentAnchorConfig>,
    /// 结算后的付款恢复策略。
    pub recovery: AuctionRecoveryPolicyConfig,
}

impl Default for AuctionSettlementConfig {
    fn default() -> Self {
        let policy = SettlementPolicy::default();
        Self {
            version: policy.version,
            mode: "hold_curve_v1".to_owned(),
            compensation_pool_enabled: policy.compensation_pool_enabled,
            isekai_carrier_credit: policy.isekai_carrier_credit,
            anchors: policy
                .anchors
                .into_iter()
                .map(|anchor| AuctionAdjustmentAnchorConfig {
                    hold_ppm: anchor.hold_ppm,
                    adjustment_ppm: anchor.adjustment_ppm,
                })
                .collect(),
            recovery: AuctionRecoveryPolicyConfig::default(),
        }
    }
}

/// 结算曲线中的一个整数锚点。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuctionAdjustmentAnchorConfig {
    /// 已连续领先时长的百万分比。
    pub hold_ppm: i64,
    /// 对最终金额应用的调整百万分比。
    pub adjustment_ppm: i64,
}

/// 恢复策略配置；当前领域仅接受七天、一次提醒的 remind_v1。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionRecoveryPolicyConfig {
    /// 恢复策略版本；省略时为 1。
    pub version: u32,
    /// 恢复模式；当前只能是 remind_v1。
    pub mode: String,
    /// 付款超时秒数；当前默认七天。
    pub payment_timeout_seconds: i64,
    /// 提醒次数；当前默认一次。
    pub reminder_limit: u32,
}

impl Default for AuctionRecoveryPolicyConfig {
    fn default() -> Self {
        Self {
            version: 1,
            mode: "remind_v1".to_owned(),
            payment_timeout_seconds: 7 * 24 * 60 * 60,
            reminder_limit: 1,
        }
    }
}
