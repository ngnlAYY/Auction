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
/// 迁移 `000077` 只提供候选索引；派生查询与证明核验实现前不得从此列表移除。
pub const SLI_AWAITING_INDEX: [&str; 0] = [];

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

/// 历史窗口 SLI 的固定流程闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliFlow {
    RegisterKey,
    SettlementFirstPaymentMail,
    SettlementLastPaymentMail,
    BuyerPaymentToSellerPayout,
    SubmissionTake,
    SubmissionRejectionNotice,
    SlowProxyExecution,
}

impl SliFlow {
    pub const ALL: [Self; 7] = [
        Self::RegisterKey,
        Self::SettlementFirstPaymentMail,
        Self::SettlementLastPaymentMail,
        Self::BuyerPaymentToSellerPayout,
        Self::SubmissionTake,
        Self::SubmissionRejectionNotice,
        Self::SlowProxyExecution,
    ];

    pub const fn index(self) -> usize {
        match self {
            Self::RegisterKey => 0,
            Self::SettlementFirstPaymentMail => 1,
            Self::SettlementLastPaymentMail => 2,
            Self::BuyerPaymentToSellerPayout => 3,
            Self::SubmissionTake => 4,
            Self::SubmissionRejectionNotice => 5,
            Self::SlowProxyExecution => 6,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::RegisterKey => "register_key",
            Self::SettlementFirstPaymentMail => "settlement_first_payment_mail",
            Self::SettlementLastPaymentMail => "settlement_last_payment_mail",
            Self::BuyerPaymentToSellerPayout => "buyer_payment_to_seller_payout",
            Self::SubmissionTake => "submission_take",
            Self::SubmissionRejectionNotice => "submission_rejection_notice",
            Self::SlowProxyExecution => "slow_proxy_execution",
        }
    }
}

/// 一个流程在七天窗口内的有界分位与覆盖结果。
#[derive(Debug, Clone, Copy, Default)]
pub struct SliFlowWindow {
    pub samples: u64,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
    /// 已识别但无法归入完整观察的候选数；不得当作零延迟。
    pub unknown: u64,
    /// 已识别但窗口或关联不完整的候选数。
    pub incomplete: u64,
    /// 未完成群组中最老者的秒数；没有已定义群组时为空。
    pub oldest_incomplete_seconds: Option<f64>,
    /// `incomplete`/`oldest` 是否有完整候选路径；false 表示覆盖不可用，不得按零解释。
    pub incomplete_available: bool,
}

/// 提醒渠道闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliOutbidChannel {
    Mm,
    Pm,
}

impl SliOutbidChannel {
    pub const ALL: [Self; 2] = [Self::Mm, Self::Pm];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Pm => "pm",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::Mm => 0,
            Self::Pm => 1,
        }
    }
}

/// 截止状态闭集；只有 final 用于 D4 验收。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliDeadlineState {
    Provisional,
    Final,
}

impl SliDeadlineState {
    pub const ALL: [Self; 2] = [Self::Provisional, Self::Final];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Provisional => "provisional",
            Self::Final => "final",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::Provisional => 0,
            Self::Final => 1,
        }
    }
}

/// 提醒送达时距有效截止的余量窗口。
#[derive(Debug, Clone, Copy, Default)]
pub struct SliOutbidMargin {
    pub samples: u64,
    pub p01: Option<f64>,
    pub p05: Option<f64>,
    pub non_positive: u64,
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
    /// 槽位依次对应 SliWorld::ALL 与 SliFlow::ALL。
    pub windows: [[SliFlowWindow; SliFlow::ALL.len()]; SliWorld::ALL.len()],
    /// 槽位依次对应 SliWorld、SliOutbidChannel 与 SliDeadlineState。
    pub outbid: [[[SliOutbidMargin; SliDeadlineState::ALL.len()]; SliOutbidChannel::ALL.len()];
        SliWorld::ALL.len()],
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
