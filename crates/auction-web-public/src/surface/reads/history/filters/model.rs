//! 价格历史筛选器的单位和错误模型。

use crate::api::PriceHistoryQuery;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryFilterError {
    Duplicate,
    WorldInvalid,
    PlayerLength,
    PlayerInvalid,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::surface::reads::history) enum HistoryField {
    World,
    Name,
    From,
    To,
    Minimum,
    Maximum,
    Buyer,
    Seller,
}

impl HistoryField {
    pub(in crate::surface::reads::history) const ALL: [Self; 8] = [
        Self::World,
        Self::Name,
        Self::From,
        Self::To,
        Self::Minimum,
        Self::Maximum,
        Self::Buyer,
        Self::Seller,
    ];

    pub(in crate::surface::reads::history) fn key(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Name => "name",
            Self::From => "from",
            Self::To => "to",
            Self::Minimum => "min_price",
            Self::Maximum => "max_price",
            Self::Buyer => "buyer",
            Self::Seller => "seller",
        }
    }

    pub(in crate::surface::reads::history) fn value(
        self,
        query: &PriceHistoryQuery,
    ) -> Option<&str> {
        match self {
            Self::World => query.world.as_deref(),
            Self::Name => query.name.as_deref(),
            Self::From => query.from.as_deref(),
            Self::To => query.to.as_deref(),
            Self::Minimum => query.min_price.as_deref(),
            Self::Maximum => query.max_price.as_deref(),
            Self::Buyer => query.buyer.as_deref(),
            Self::Seller => query.seller.as_deref(),
        }
    }

    pub(super) fn focus_id(self) -> &'static str {
        match self {
            Self::World => "public-history-world",
            Self::Name => "public-history-name",
            Self::From => "public-history-from-year",
            Self::To => "public-history-to-year",
            Self::Minimum => "public-history-min-price",
            Self::Maximum => "public-history-max-price",
            Self::Buyer => "public-history-buyer",
            Self::Seller => "public-history-seller",
        }
    }

    /// 日期子字段的焦点目标；月份与年份可分别定位。
    pub(super) fn month_focus_id(self) -> &'static str {
        match self {
            Self::From => "public-history-from-month",
            Self::To => "public-history-to-month",
            _ => self.focus_id(),
        }
    }

    pub(super) fn partner(self) -> Option<Self> {
        match self {
            Self::From => Some(Self::To),
            Self::To => Some(Self::From),
            Self::Minimum => Some(Self::Maximum),
            Self::Maximum => Some(Self::Minimum),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::surface::reads::history) struct HistoryErrors([Option<HistoryFilterError>; 8]);

impl HistoryErrors {
    pub(in crate::surface::reads::history) fn get(
        self,
        field: HistoryField,
    ) -> Option<HistoryFilterError> {
        self.0[field as usize]
    }

    pub(in crate::surface::reads::history) fn set(
        &mut self,
        field: HistoryField,
        error: Option<HistoryFilterError>,
    ) {
        self.0[field as usize] = error;
    }

    pub(in crate::surface::reads::history) fn first(self) -> Option<HistoryField> {
        HistoryField::ALL.into_iter().find(|field| self.get(*field).is_some())
    }

    pub(super) fn in_settings(self) -> bool {
        HistoryField::ALL.into_iter().any(|field| {
            !matches!(field, HistoryField::Name | HistoryField::World) && self.get(field).is_some()
        })
    }
}

impl HistoryFilterError {
    pub(super) fn is_range(self) -> bool {
        matches!(self, Self::DateRange | Self::PriceRange)
    }
}
