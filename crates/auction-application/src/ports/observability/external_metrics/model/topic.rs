//! 论坛主题扫描指标闭集：扫描阶段、页数口径与终结原因。

/// 主题扫描阶段：完整读取或按游标增量读取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumTopicScanMode {
    Full,
    Incremental,
}

impl ForumTopicScanMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

/// 主题扫描页数口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumTopicScanMeasure {
    Discovered,
    Processed,
}

impl ForumTopicScanMeasure {
    pub const ALL: [Self; 2] = [Self::Discovered, Self::Processed];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Processed => "processed",
        }
    }
}

/// 主题扫描终结原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumTopicScanOutcome {
    Complete,
    PageLimit,
    Error,
    Canceled,
}

impl ForumTopicScanOutcome {
    pub const ALL: [Self; 4] = [Self::Complete, Self::PageLimit, Self::Error, Self::Canceled];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::PageLimit => "page_limit",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }
}
