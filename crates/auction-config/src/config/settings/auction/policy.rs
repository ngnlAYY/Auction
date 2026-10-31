//! 拍卖创建时冻结的经济、结算和恢复策略配置。

use chrono::Duration;
use serde::{Deserialize, Serialize};

use super::super::ConfigError;
use auction_application::{
    AuctionEarlyBirdConfig, AuctionPolicyTemplate, AuctionScheduleCalendar, AuctionWeeklySchedule,
};
use auction_domain::{
    AdjustmentAnchor, DEFAULT_COD_RETURN_LIMIT, RecoveryMode, RecoveryPolicy, SettlementMode,
    SettlementPolicy,
};

/// 拍卖创建时冻结的非敏感策略配置。
/// 该区块对应旧版共享配置中的 `auction.economics` 与 `auction.settlement`。所有字段都可
/// 省略；应用层会在创建事务中把已校验模板同时写入 JSON 快照和遗留规则列。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionConfig {
    pub economics: AuctionEconomicsConfig,
    pub settlement: AuctionSettlementConfig,
    /// 每周轮次生成、preview/open 和 MoogleMail gate 的稳定计划。
    pub schedule: AuctionScheduleConfig,
}

/// 使用 IANA 时区表达的本地周拍卖时间表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionScheduleConfig {
    /// 是否启用 Worker 周计划；省略时关闭，避免开发数据库自动建轮次。
    pub enabled: bool,
    pub timezone: String,
    /// preview 的星期名称，例如 Friday。
    pub preview_day: String,
    /// preview 的本地时间，格式 HH:MM。
    pub preview_time: String,
    pub open_day: String,
    pub open_time: String,
    pub nominal_end_day: String,
    pub nominal_end_time: String,
    /// 参与周计划的 world；省略时使用 main 和 isekai。
    pub worlds: Vec<String>,
    /// 赛季早鸟期：窗口内替代常规周场、单独收件品质并在赛季前暂停收件；默认关闭。
    pub early_bird: AuctionEarlyBirdConfig,
}

impl Default for AuctionScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timezone: "UTC".to_owned(),
            preview_day: "Thursday".to_owned(),
            preview_time: "22:00".to_owned(),
            open_day: "Friday".to_owned(),
            open_time: "00:00".to_owned(),
            nominal_end_day: "Saturday".to_owned(),
            nominal_end_time: "12:00".to_owned(),
            worlds: vec!["main".to_owned(), "isekai".to_owned()],
            early_bird: AuctionEarlyBirdConfig::default(),
        }
    }
}

impl AuctionConfig {
    /// 周排期启用时返回 Admin/Worker 共用的排期日历；早鸟期仅在其自身也启用时加入。
    /// 调用方须先完成配置校验；这里仍独立失败关闭，避免装配根拿到半解析的日历。
    pub fn schedule_calendar(&self) -> Result<Option<AuctionScheduleCalendar>, ConfigError> {
        let schedule = &self.schedule;
        if !schedule.enabled {
            return Ok(None);
        }
        let weekly = AuctionWeeklySchedule::from_names(
            &schedule.timezone,
            (&schedule.preview_day, &schedule.preview_time),
            (&schedule.open_day, &schedule.open_time),
            (&schedule.nominal_end_day, &schedule.nominal_end_time),
        )
        .map_err(|_| ConfigError::InvalidAuctionConfig("auction.schedule 时间配置无效"))?;
        let early_bird = schedule.early_bird.active_calendar().map_err(|_| {
            ConfigError::InvalidAuctionConfig("auction.schedule.early_bird 字段无效")
        })?;
        Ok(Some(AuctionScheduleCalendar::new(weekly, early_bird)))
    }

    /// 将 TOML 设置转换为应用层策略模板；模式不在闭集或秒数超出 chrono 范围时返回配置错误。
    pub fn policy(&self) -> Result<AuctionPolicyTemplate, ConfigError> {
        // 此公开转换也可能在主配置校验之前调用，模式和时间转换都必须独立失败关闭。
        let duration = |seconds| {
            Duration::try_seconds(seconds)
                .ok_or(ConfigError::InvalidAuctionConfig("策略秒数超出可表示范围"))
        };
        let settlement_mode = SettlementMode::from_name(&self.settlement.mode).ok_or(
            ConfigError::InvalidAuctionConfig(
                "settlement.mode 只能是 legacy_zero_v0、hold_curve_v1 或 hold_curve_v2",
            ),
        )?;
        let recovery_mode = match self.settlement.recovery.mode.as_str() {
            "remind_v1" => RecoveryMode::RemindV1,
            _ => {
                return Err(ConfigError::InvalidAuctionConfig(
                    "settlement.recovery.mode 只能是 remind_v1",
                ));
            }
        };
        Ok(AuctionPolicyTemplate {
            start_price: self.economics.start_price,
            bid_increment_ppm: self.economics.bid_increment_ppm,
            soft_close_window: duration(self.economics.soft_close_window_seconds)?,
            soft_close_extension: duration(self.economics.soft_close_extension_seconds)?,
            proxy_enabled: self.economics.proxy_enabled,
            proxy_default_mode: self.economics.proxy_default_mode.clone(),
            proxy_slow_interval: duration(self.economics.proxy_slow_interval_seconds)?,
            proxy_max_outbid_uses: self.economics.proxy_max_outbid_uses,
            final_phase_lead: duration(self.economics.final_phase_lead_seconds)?,
            cancel_cutoff: duration(self.economics.cancel_cutoff_seconds)?,
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
                payment_timeout: duration(self.settlement.recovery.payment_timeout_seconds)?,
                reminder_limit: self.settlement.recovery.reminder_limit,
                cod_return_limit: self.settlement.recovery.cod_return_limit,
            },
        })
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
    /// 软截止窗口秒数，不能为负；省略时为 900 秒。
    pub soft_close_window_seconds: i64,
    /// 软截止延长秒数，必须大于 0；省略时为 900 秒。
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
    /// final phase 领先秒数，范围 1-604800；省略时为 900 秒。
    pub final_phase_lead_seconds: i64,
}

impl Default for AuctionEconomicsConfig {
    fn default() -> Self {
        Self {
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

/// 结算策略配置；锚点顺序和闭集模式由应用/领域校验。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionSettlementConfig {
    /// 新模板结算策略版本为 0 或 2，省略时为 2；1 仅用于历史快照解码。
    pub version: u32,
    /// 新模板使用 hold_curve_v2 或 legacy_zero_v0，默认 hold_curve_v2；v1 不得用于新拍卖。
    pub mode: String,
    /// 是否启用补偿池；省略时启用。
    pub compensation_pool_enabled: bool,
    /// isekai carrier credit 数量；当前策略要求为 1。
    pub isekai_carrier_credit: u32,
    /// 持有时长与调整比例的 PPM 锚点；省略时使用内置 hold curve。
    pub anchors: Vec<AuctionAdjustmentAnchorConfig>,
    pub recovery: AuctionRecoveryPolicyConfig,
}

impl Default for AuctionSettlementConfig {
    fn default() -> Self {
        let policy = SettlementPolicy::default();
        Self {
            version: policy.version,
            mode: "hold_curve_v2".to_owned(),
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

/// 恢复策略配置；领域只接受一次提醒的 remind_v1，版本 1 为七天、版本 2 为 24 小时付款期限。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuctionRecoveryPolicyConfig {
    /// 恢复策略版本；省略时为 1。
    pub version: u32,
    /// 恢复模式；当前只能是 remind_v1。
    pub mode: String,
    /// 付款超时秒数；须与版本对应，默认七天。
    pub payment_timeout_seconds: i64,
    /// 提醒次数；当前默认一次。
    pub reminder_limit: u32,
    /// 同一 recovery 允许的 COD 退回次数；范围 1-10，默认 3。
    pub cod_return_limit: u32,
}

impl Default for AuctionRecoveryPolicyConfig {
    fn default() -> Self {
        Self {
            version: 1,
            mode: "remind_v1".to_owned(),
            payment_timeout_seconds: 7 * 24 * 60 * 60,
            reminder_limit: 1,
            cod_return_limit: DEFAULT_COD_RETURN_LIMIT,
        }
    }
}
