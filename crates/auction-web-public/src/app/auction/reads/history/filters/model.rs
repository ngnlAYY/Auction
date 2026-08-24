//! 价格历史筛选器的单位和错误模型。

/// 价格输入使用旧版页面的千（K）或百万（M）单位。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryPriceUnit {
    K,
    M,
}

impl HistoryPriceUnit {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::K => "K",
            Self::M => "M",
        }
    }

    pub(super) fn from_str(value: &str) -> Self {
        if value == "M" { Self::M } else { Self::K }
    }
}

/// 日期筛选输入的局部错误。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryDateError {
    Incomplete,
    Invalid,
}

/// 价格筛选输入的局部错误。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryPriceError {
    Invalid,
}

/// 价格历史筛选器向页面反馈的稳定错误分类。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryFilterError {
    FromDateIncomplete,
    FromDateInvalid,
    ToDateIncomplete,
    ToDateInvalid,
    DateRange,
    MinimumPriceK,
    MinimumPriceM,
    MaximumPriceK,
    MaximumPriceM,
    PriceRange,
}
