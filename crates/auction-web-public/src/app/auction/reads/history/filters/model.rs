//! 价格历史筛选器的单位和错误模型。

/// 价格输入支持 Credits（C）、千（K）和百万（M），最终均提交整数 Credits。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryPriceUnit {
    C,
    K,
    M,
}

impl HistoryPriceUnit {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::C => "C",
            Self::K => "K",
            Self::M => "M",
        }
    }

    pub(super) fn from_str(value: &str) -> Self {
        match value {
            "C" => Self::C,
            "M" => Self::M,
            _ => Self::K,
        }
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
    NameTooManyTerms,
    NameTermTooLong,
    NameTooLong,
    NameInvalid,
    FromDateIncomplete,
    FromDateInvalid,
    ToDateIncomplete,
    ToDateInvalid,
    DateRange,
    MinimumPriceC,
    MinimumPriceK,
    MinimumPriceM,
    MaximumPriceC,
    MaximumPriceK,
    MaximumPriceM,
    PriceRange,
}
