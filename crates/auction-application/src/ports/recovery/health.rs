//! 恢复健康的安全状态与刷新端口；记账模式明确区分现金不适用和证据读取失败。

use std::time::Duration;

use chrono::{DateTime, Utc};

use auction_domain::SettlementWorld;

use super::ApplicationFuture;

/// recovery health 中余额来源的有限状态集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryBalanceStatus {
    /// 记账模式不采集或比较账号余额。
    NotApplicable,
    Unknown,
    Ok,
    Low,
    Stale,
    Busy,
    Unavailable,
}

impl RecoveryBalanceStatus {
    /// 返回迁移表和运维投影使用的稳定状态名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Unknown => "unknown",
            Self::Ok => "ok",
            Self::Low => "low",
            Self::Stale => "stale",
            Self::Busy => "busy",
            Self::Unavailable => "unavailable",
        }
    }
}

/// recovery health 中外部证据的有限状态集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryEvidenceStatus {
    /// 当前健康来源为现金观察，记账模式明确不适用。
    NotApplicable,
    Unknown,
    Fresh,
    Stale,
    Unavailable,
}

impl RecoveryEvidenceStatus {
    /// 返回迁移表和运维投影使用的稳定状态名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Unknown => "unknown",
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
        }
    }
}

/// 单个 world 的 recovery health 持久化更新。
/// 余额和最后成功证据允许为空；为空表示本轮外部来源没有提供新事实，存储 owner 必须保留
/// 数据库中的最后成功值，不能把失败读取伪装成零余额。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryHealthUpdate {
    pub world: SettlementWorld,
    pub balance_credits: Option<i64>,
    pub balance_observed_at: Option<DateTime<Utc>>,
    pub balance_status: RecoveryBalanceStatus,
    pub evidence_last_success_at: Option<DateTime<Utc>>,
    pub evidence_status: RecoveryEvidenceStatus,
    pub updated_at: DateTime<Utc>,
}

impl RecoveryHealthUpdate {
    /// 校验可写入数据库的状态组合和时间边界。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.updated_at.timestamp_nanos_opt().is_none() {
            return Err("recovery health 更新时间无效");
        }
        let has_balance = self.balance_credits.is_some() || self.balance_observed_at.is_some();
        if (self.balance_status == RecoveryBalanceStatus::NotApplicable)
            != (self.evidence_status == RecoveryEvidenceStatus::NotApplicable)
        {
            return Err("recovery health 现金不适用状态必须成对出现");
        }
        if has_balance != (self.balance_credits.is_some() && self.balance_observed_at.is_some())
            || self.balance_credits.is_some_and(|credits| credits < 0)
            || self.balance_observed_at.is_some_and(|observed_at| {
                observed_at.timestamp_nanos_opt().is_none() || observed_at > self.updated_at
            })
            || self.evidence_last_success_at.is_some_and(|observed_at| {
                observed_at.timestamp_nanos_opt().is_none() || observed_at > self.updated_at
            })
        {
            return Err("recovery health 观测值无效");
        }

        match self.balance_status {
            RecoveryBalanceStatus::Ok
            | RecoveryBalanceStatus::Low
            | RecoveryBalanceStatus::Stale
                if !has_balance =>
            {
                return Err("recovery health 余额状态缺少观测值");
            }
            RecoveryBalanceStatus::NotApplicable
            | RecoveryBalanceStatus::Unknown
            | RecoveryBalanceStatus::Busy
            | RecoveryBalanceStatus::Unavailable
                if has_balance =>
            {
                return Err("recovery health 不可用余额状态不应携带观测值");
            }
            _ => {}
        }

        match self.evidence_status {
            RecoveryEvidenceStatus::Fresh | RecoveryEvidenceStatus::Stale
                if self.evidence_last_success_at.is_none() =>
            {
                return Err("recovery health 证据状态缺少成功时间");
            }
            RecoveryEvidenceStatus::NotApplicable
            | RecoveryEvidenceStatus::Unknown
            | RecoveryEvidenceStatus::Unavailable
                if self.evidence_last_success_at.is_some() =>
            {
                return Err("recovery health 不可用证据状态不应携带成功时间");
            }
            _ => {}
        }
        Ok(())
    }
}

/// recovery health 的数据库 owner 端口。
pub trait RecoveryHealthStore: Send + Sync {
    /// 刷新一个 world，并在同一 SQL 写入中重算 recovery backlog 计数。
    fn refresh_world_health(&self, update: RecoveryHealthUpdate) -> ApplicationFuture<'_, ()>;
}

pub trait RecoveryHealthRefreshPort: Send + Sync {
    /// 按持久资金策略与 world 集合刷新安全投影；记账模式仅刷新非现金计数。
    fn refresh(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, ()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryHealthConfig {
    pub worlds: Vec<SettlementWorld>,
    pub low_balance_threshold: i64,
    pub evidence_stale_after: Duration,
}

impl RecoveryHealthConfig {
    /// 校验 world 闭集和健康阈值，避免把无界配置传入 Worker。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.worlds.is_empty()
            || self.worlds.len() > 2
            || self.low_balance_threshold < 0
            || self.evidence_stale_after.is_zero()
            || self.worlds.windows(2).any(|worlds| worlds[0] == worlds[1])
        {
            return Err("recovery health 配置无效");
        }
        Ok(())
    }
}
