//! 回帖延迟估计的闭集：拍卖阶段、估计方法、上下界与采样结果。

/// 导入时观察到的拍卖阶段；未知不与 normal/final 合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyDelayPhase {
    Normal,
    Final,
    Unknown,
}

impl ReplyDelayPhase {
    pub const ALL: [Self; 3] = [Self::Normal, Self::Final, Self::Unknown];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Final => "final",
            Self::Unknown => "unknown",
        }
    }
}

/// 回帖延迟估计方法；posted_minute 留待后续核实后扩展。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyDelayMethod {
    PollWindow,
}

impl ReplyDelayMethod {
    pub const fn label(self) -> &'static str {
        match self {
            Self::PollWindow => "poll_window",
        }
    }
}

/// 延迟估计上下界字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyDelayBound {
    Lower,
    Midpoint,
    Upper,
}

impl ReplyDelayBound {
    pub const ALL: [Self; 3] = [Self::Lower, Self::Midpoint, Self::Upper];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lower => "lower",
            Self::Midpoint => "midpoint",
            Self::Upper => "upper",
        }
    }
}

/// 延迟采样覆盖结果；缺样不是零延迟。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyDelaySampleOutcome {
    Observed,
    MissingBaseline,
    CoverageUnknown,
    StateEvicted,
    TimestampUnavailable,
    ClockInvalid,
}

impl ReplyDelaySampleOutcome {
    pub const ALL: [Self; 6] = [
        Self::Observed,
        Self::MissingBaseline,
        Self::CoverageUnknown,
        Self::StateEvicted,
        Self::TimestampUnavailable,
        Self::ClockInvalid,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::MissingBaseline => "missing_baseline",
            Self::CoverageUnknown => "coverage_unknown",
            Self::StateEvicted => "state_evicted",
            Self::TimestampUnavailable => "timestamp_unavailable",
            Self::ClockInvalid => "clock_invalid",
        }
    }
}
