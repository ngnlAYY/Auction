//! 拍卖创建时冻结的策略模板。
//! 该类型是配置适配器与拍卖创建用例之间的窄边界。它不读取 TOML，也不包含数据库字段；
//! 应用层只负责校验策略并把同一份不可变模板交给 PostgreSQL 事务，避免配置快照和竞价
//! 所依赖的遗留规则列出现分叉。

use auction_domain::{
    AuctionRules, BidAmount, DEFAULT_BID_INCREMENT_PPM, DEFAULT_CANCEL_CUTOFF,
    DEFAULT_FINAL_PHASE_LEAD, DEFAULT_PROXY_MAX_OUTBID_USES, DEFAULT_PROXY_SLOW_INTERVAL,
    DEFAULT_SOFT_CLOSE_EXTENSION, DEFAULT_SOFT_CLOSE_WINDOW, DEFAULT_START_PRICE, DomainError,
    ProxyMode, RecoveryPolicy, SettlementPolicy,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

/// 策略快照中沿用旧契约的固定最低增量。
pub const POLICY_MINIMUM_INCREMENT: i64 = 30_000;

/// 拍卖创建时复制进 `policy_snapshot` 的策略模板。
/// 所有经济、代理和 final phase 字段都会随拍卖冻结；竞价事务从同一快照恢复规则，配置变更
/// 不会悄悄改变既有拍卖。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionPolicyTemplate {
    /// 拍卖默认起始价，必须为正且按论坛信用网格对齐。
    pub start_price: i64,
    /// 旧版经济策略使用的百万分比增量。
    pub bid_increment_ppm: i64,
    /// 软截止触发窗口。
    pub soft_close_window: Duration,
    /// 软截止延长时长。
    pub soft_close_extension: Duration,
    /// 是否允许代理竞价。
    pub proxy_enabled: bool,
    /// 新代理授权的默认模式，只允许 `fast` 或 `slow`。
    pub proxy_default_mode: String,
    pub proxy_slow_interval: Duration,
    /// 单个代理授权允许的 outbid 使用次数。
    pub proxy_max_outbid_uses: u32,
    pub final_phase_lead: Duration,
    /// 取消截止窗口；当前仍由遗留 auctions 列提供给生命周期规则。
    pub cancel_cutoff: Duration,
    /// 随拍卖一起冻结的结算策略。
    pub settlement: SettlementPolicy,
    /// 随拍卖一起冻结的恢复策略。
    pub recovery: RecoveryPolicy,
}

impl Default for AuctionPolicyTemplate {
    fn default() -> Self {
        Self {
            start_price: DEFAULT_START_PRICE,
            bid_increment_ppm: DEFAULT_BID_INCREMENT_PPM,
            soft_close_window: DEFAULT_SOFT_CLOSE_WINDOW,
            soft_close_extension: DEFAULT_SOFT_CLOSE_EXTENSION,
            proxy_enabled: true,
            proxy_default_mode: "fast".to_owned(),
            proxy_slow_interval: DEFAULT_PROXY_SLOW_INTERVAL,
            proxy_max_outbid_uses: DEFAULT_PROXY_MAX_OUTBID_USES,
            final_phase_lead: DEFAULT_FINAL_PHASE_LEAD,
            cancel_cutoff: DEFAULT_CANCEL_CUTOFF,
            settlement: SettlementPolicy::default(),
            recovery: RecoveryPolicy::default(),
        }
    }
}

impl AuctionPolicyTemplate {
    /// 将模板中当前已接入竞价领域的字段转换为规则快照。
    pub fn auction_rules(&self) -> Result<AuctionRules, DomainError> {
        let proxy_default_mode = ProxyMode::from_name(&self.proxy_default_mode)
            .ok_or(DomainError::InvalidRules("代理默认模式无效"))?;
        let rules = AuctionRules {
            bidding_version: 2,
            default_start_price: BidAmount::new(self.start_price)?,
            bid_increment_ppm: self.bid_increment_ppm,
            minimum_increment: if self.bid_increment_ppm == 0 {
                POLICY_MINIMUM_INCREMENT
            } else {
                0
            },
            soft_close_window: self.soft_close_window,
            soft_close_extension: self.soft_close_extension,
            cancel_cutoff: self.cancel_cutoff,
            proxy_slow_interval: self.proxy_slow_interval,
            proxy_enabled: self.proxy_enabled,
            proxy_default_mode,
            proxy_max_outbid_uses: self.proxy_max_outbid_uses,
            final_phase_lead: self.final_phase_lead,
        };
        rules.validate()?;
        Ok(rules)
    }

    /// 赛季前场次冻结的模板：恢复策略换成 24 小时付款期限的 v2，其余字段与当前模板一致。
    pub fn for_pre_season(&self) -> Self {
        Self { recovery: self.recovery.pre_season(), ..self.clone() }
    }

    /// 校验配置闭集和结算/恢复策略，防止未识别参数进入不可变快照。
    pub fn validate(&self) -> Result<(), DomainError> {
        self.auction_rules()?;
        if !(0..=1_000_000).contains(&self.bid_increment_ppm)
            || !matches!(self.proxy_default_mode.as_str(), "fast" | "slow")
            || self.final_phase_lead <= Duration::zero()
            || self.final_phase_lead > Duration::days(7)
            || self.proxy_max_outbid_uses > 1_000
        {
            return Err(DomainError::InvalidRules("经济策略字段超出允许范围"));
        }
        self.settlement.validate_for_new_auction()?;
        self.recovery.validate()?;
        Ok(())
    }

    /// 生成 schema v5 快照，显式冻结统一最低出价的竞价策略 v2。
    /// `cancel_cutoff` 不写入当前快照 JSON，因为现有 schema 仍由 auctions 遗留列承载；创建
    /// 事务会用同一模板同时写入该列，保证生命周期读取和快照来源一致。
    pub fn snapshot(&self, scheduled_end_at: DateTime<Utc>) -> Value {
        json!({
            "schema_version": 5,
            "bidding_version": 2,
            "start_price": self.start_price,
            "bid_increment_ppm": self.bid_increment_ppm,
            "minimum_increment": if self.bid_increment_ppm == 0 {
                POLICY_MINIMUM_INCREMENT
            } else {
                0
            },
            "soft_close_window_seconds": self.soft_close_window.num_seconds(),
            "soft_close_extension_seconds": self.soft_close_extension.num_seconds(),
            "proxy_enabled": self.proxy_enabled,
            "proxy_default_mode": self.proxy_default_mode,
            "proxy_slow_interval_seconds": self.proxy_slow_interval.num_seconds(),
            "proxy_max_outbid_uses": self.proxy_max_outbid_uses,
            "final_phase_lead_seconds": self.final_phase_lead.num_seconds(),
            "scheduled_end_at": scheduled_end_at.to_rfc3339(),
            "settlement": {
                "version": self.settlement.version,
                "mode": self.settlement.mode.as_str(),
                "compensation_pool_enabled": self.settlement.compensation_pool_enabled,
                "isekai_carrier_credit": self.settlement.isekai_carrier_credit,
                "anchors": self.settlement.anchors,
            },
            "recovery": {
                "version": self.recovery.version,
                "mode": self.recovery.mode.as_str(),
                "payment_timeout_seconds": self.recovery.payment_timeout.num_seconds(),
                "reminder_limit": self.recovery.reminder_limit,
                "cod_return_limit": self.recovery.cod_return_limit,
            },
        })
    }
}
