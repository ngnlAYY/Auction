//! 地址筛选的缺省、合法选择和待修正原文。

use auction_contracts::EQUIPMENT_CATEGORY_CODES;

/// 单值筛选保留非法值及重复次数，只有缺省可以启用页面默认值。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FilterParameter {
    #[default]
    Unspecified,
    Selected(String),
    Invalid(Vec<String>),
}

impl FilterParameter {
    pub(super) fn from_values(
        values: Vec<String>,
        parse: impl FnOnce(&str) -> Option<String>,
    ) -> Self {
        match values.as_slice() {
            [] => Self::Unspecified,
            [value] => parse(value).map_or(Self::Invalid(values), Self::Selected),
            _ => Self::Invalid(values),
        }
    }

    pub(crate) fn from_categories(values: Vec<String>) -> Self {
        Self::from_values(values, |value| is_catalog_category(value).then(|| value.to_owned()))
    }

    pub(crate) fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Selected(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn query_values(&self) -> &[String] {
        match self {
            Self::Unspecified => &[],
            Self::Selected(value) => std::slice::from_ref(value),
            Self::Invalid(values) => values,
        }
    }

    pub(crate) fn invalid(&self) -> bool {
        matches!(self, Self::Invalid(_))
    }

    /// 跨世界读取页的实际范围。非法书签不生成请求范围。
    pub(crate) fn round_world(&self) -> Option<&str> {
        match self {
            Self::Unspecified => Some("main"),
            Self::Selected(world) => Some(world),
            Self::Invalid(_) => None,
        }
    }

    pub(crate) fn catalog_category(&self) -> Option<Option<&str>> {
        match self {
            Self::Unspecified => Some(None),
            Self::Selected(category) if is_catalog_category(category) => Some(Some(category)),
            Self::Selected(_) | Self::Invalid(_) => None,
        }
    }
}

impl From<Option<String>> for FilterParameter {
    fn from(value: Option<String>) -> Self {
        value.map_or(Self::Unspecified, Self::Selected)
    }
}

fn is_catalog_category(value: &str) -> bool {
    EQUIPMENT_CATEGORY_CODES.iter().any(|(category, _, _)| *category == value)
}
