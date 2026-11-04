//! 只读 SLI 快照端口；仅传递有界数值，不承载业务判定或标识。

use std::{future::Future, pin::Pin, time::Duration};

use chrono::{DateTime, Utc};

/// 完成窗口固定七天；未完成慢速代理另作当前积压快照。
pub const SLI_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;
/// 索引候选读取上限；超过上限整轮不可评估，不发布截断统计。
pub const SLI_CANDIDATE_LIMIT: usize = 10_000;
/// 慢速代理当前积压的固定 flow 标签。
pub const SLI_SLOW_PROXY_FLOW: &str = "slow_proxy_execution";
/// 尚缺窗口候选索引的历史流程；不得导出伪造的零分位。
pub const SLI_AWAITING_INDEX: [&str; 8] = [
    "register_key",
    "outbid_deadline_margin",
    "settlement_first_payment_mail",
    "settlement_last_payment_mail",
    "buyer_payment_to_seller_payout",
    "submission_take",
    "submission_rejection_notice",
    SLI_SLOW_PROXY_FLOW,
];

/// 指标支持的固定世界集合。
#[derive(Debug, Clone, Copy)]
pub enum SliWorld {
    Main,
    Isekai,
}

impl SliWorld {
    /// 顺序只用于固定快照槽位。
    pub const ALL: [Self; 2] = [Self::Main, Self::Isekai];
    /// 世界标签不接受任意数据库字符串。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
        }
    }
}

/// 容量观测按逻辑命令区分；自动反击事实不增加命令数。
#[derive(Debug, Clone, Copy)]
pub enum SliBidKind {
    Ordinary,
    ProxyCreate,
    SlowProxy,
}

impl SliBidKind {
    /// 固定快照槽位。
    pub const ALL: [Self; 3] = [Self::Ordinary, Self::ProxyCreate, Self::SlowProxy];
    /// 固定容量标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::ProxyCreate => "proxy_create",
            Self::SlowProxy => "slow_proxy",
        }
    }
}

/// 一类 Final 命令的有界窗口统计。
#[derive(Debug, Clone, Copy, Default)]
pub struct SliBidCapacity {
    /// 最近六十秒已接受的逻辑命令数除以六十。
    pub per_second: f64,
    /// 七天窗口内按数据库事件创建秒分桶的最大命令数。
    pub peak_per_second: u64,
}

impl SliBidCapacity {
    /// 持续吞吐的固定观察长度，与峰值的单秒桶分开。
    pub const RATE_WINDOW_SECONDS: i64 = 60;
}

/// 当前持久化到期调度的积压；不猜测失败或已清除世代。
#[derive(Debug, Clone, Copy, Default)]
pub struct SliPending {
    /// 当前到期条目数。
    pub count: u64,
    /// 最老到期时间至数据库快照时间的秒数。
    pub oldest_seconds: f64,
}

/// 一轮只读事务内取得的数值快照；不含任何玩家或业务主键。
#[derive(Debug, Clone)]
pub struct SliSnapshot {
    /// 数据库快照时刻。
    pub observed_at: DateTime<Utc>,
    /// 槽位依次对应 SliWorld::ALL 与 SliBidKind::ALL。
    pub final_bids: [[SliBidCapacity; SliBidKind::ALL.len()]; SliWorld::ALL.len()],
    /// 每个世界当前尚留在 lot 上的到期慢速代理。
    pub slow_pending: [SliPending; SliWorld::ALL.len()],
}

impl SliSnapshot {
    /// 包含 BEGIN、全部查询与确认回滚的单轮总预算。
    pub const TIMEOUT: Duration = Duration::from_secs(2);
}

/// 闭集失败原因；不能以超时或截断样本证明完整窗口。
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum SliSnapshotError {
    #[error("连接池忙，本轮跳过")]
    Busy,
    #[error("只读汇总超时")]
    Timeout,
    #[error("候选超过固定上限")]
    Limited,
    #[error("只读汇总存储不可用")]
    Storage,
    #[error("只读汇总数据无效")]
    InvalidData,
}

impl SliSnapshotError {
    /// 固定诊断与刷新结果标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Timeout => "timeout",
            Self::Limited => "limited",
            Self::Storage => "storage",
            Self::InvalidData => "invalid_data",
        }
    }
}

/// storage 单一查询 owner 的异步只读边界。
pub trait SliSnapshotReader: Send + Sync {
    /// 一轮串行最多借一个空闲连接；失败不提供部分快照。
    fn read_sli_snapshot(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<SliSnapshot, SliSnapshotError>> + Send + '_>>;
}
