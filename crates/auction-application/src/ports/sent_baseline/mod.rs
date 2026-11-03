//! 发送前 Sent 边界的配置与闭集观测端口；不承载游标、完成状态或发送授权。

use std::sync::Arc;

use serde::Deserialize;

/// 发送前基线策略；影子模式只由旧路径裁决。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SentBaselineMode {
    /// 完整排除集与既有缓存。
    Full,
    /// 旧证明裁决，同时只读比较水位候选。
    #[default]
    Shadow,
    /// 每次发送前 fresh 捕获临时数值 MID 边界。
    Watermark,
}

/// 可比较影子样本的实际分支。
#[derive(Debug, Clone, Copy)]
pub enum SentBaselineBranch {
    Empty,
    NewMinute,
    SameMinute,
    SameMinuteCrossPage,
    NoCandidate,
    MultipleCandidates,
    FullFallback,
}

impl SentBaselineBranch {
    /// 指标预置的闭集。
    pub const ALL: [Self; 7] = [
        Self::Empty,
        Self::NewMinute,
        Self::SameMinute,
        Self::SameMinuteCrossPage,
        Self::NoCandidate,
        Self::MultipleCandidates,
        Self::FullFallback,
    ];

    /// 不含远端身份的固定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::NewMinute => "new_minute",
            Self::SameMinute => "same_minute",
            Self::SameMinuteCrossPage => "same_minute_cross_page",
            Self::NoCandidate => "no_candidate",
            Self::MultipleCandidates => "multiple_candidates",
            Self::FullFallback => "full_fallback",
        }
    }
}

/// 不可比较或完整回退的闭集原因；数字空洞不是不连续。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentBaselineReason {
    InvalidMid,
    OrderViolation,
    PageDiscontinuity,
    AnchorMissing,
    AnchorChanged,
    PageLimit,
    Timeout,
    ReadUnavailable,
    CacheBoundaryDelta,
    BoundaryUnavailable,
    ScopeChanged,
    ResponseHintConflict,
    Canceled,
}

impl SentBaselineReason {
    /// 指标预置的闭集。
    pub const ALL: [Self; 13] = [
        Self::InvalidMid,
        Self::OrderViolation,
        Self::PageDiscontinuity,
        Self::AnchorMissing,
        Self::AnchorChanged,
        Self::PageLimit,
        Self::Timeout,
        Self::ReadUnavailable,
        Self::CacheBoundaryDelta,
        Self::BoundaryUnavailable,
        Self::ScopeChanged,
        Self::ResponseHintConflict,
        Self::Canceled,
    ];

    /// 不含错误原文或远端身份的固定标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::InvalidMid => "invalid_mid",
            Self::OrderViolation => "order_violation",
            Self::PageDiscontinuity => "page_discontinuity",
            Self::AnchorMissing => "anchor_missing",
            Self::AnchorChanged => "anchor_changed",
            Self::PageLimit => "page_limit",
            Self::Timeout => "timeout",
            Self::ReadUnavailable => "read_unavailable",
            Self::CacheBoundaryDelta => "cache_boundary_delta",
            Self::BoundaryUnavailable => "boundary_unavailable",
            Self::ScopeChanged => "scope_changed",
            Self::ResponseHintConflict => "response_hint_conflict",
            Self::Canceled => "canceled",
        }
    }
}

/// 完整回退发生在首次外写之前或之后。
#[derive(Debug, Clone, Copy)]
pub enum SentBaselinePhase {
    BeforeSend,
    AfterSend,
}

impl SentBaselinePhase {
    /// 指标预置的闭集。
    pub const ALL: [Self; 2] = [Self::BeforeSend, Self::AfterSend];

    /// 固定阶段标签。
    pub const fn label(self) -> &'static str {
        match self {
            Self::BeforeSend => "before_send",
            Self::AfterSend => "after_send",
        }
    }
}

/// 仅含闭集分类与布尔比较的观测；不携带 MID、邮件内容或凭据。
#[derive(Debug, Clone, Copy)]
pub enum SentBaselineEvent {
    Evaluation { branch: SentBaselineBranch, mid_sets_equal: bool, candidate_sets_equal: bool },
    Uncomparable(SentBaselineReason),
    Fallback { phase: SentBaselinePhase, reason: SentBaselineReason },
}

/// 局部影子指标端口；观察失败不得改变发送结果。
pub trait SentBaselineObserver: Send + Sync {
    /// world 仅接受 main/isekai；适配器不能把任意字符串写入标签。
    fn observe(&self, world: &str, event: SentBaselineEvent);
}

struct NoopSentBaselineObserver;

impl SentBaselineObserver for NoopSentBaselineObserver {
    fn observe(&self, _world: &str, _event: SentBaselineEvent) {}
}

/// 未装配 exporter 时保持同一证明流程。
pub fn noop_sent_baseline_observer() -> Arc<dyn SentBaselineObserver> {
    Arc::new(NoopSentBaselineObserver)
}
