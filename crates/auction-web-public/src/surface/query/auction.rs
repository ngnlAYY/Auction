//! 场次参数的互斥状态及选择意图。

use auction_web_shared::{QueryWriteMode, valid_uuid};

use super::PublicBoardQuery;

/// 只有缺省状态可以回退到可出价的当前场次。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum AuctionParameter {
    #[default]
    Unspecified,
    Selected(String),
    Invalid(Vec<String>),
}

impl AuctionParameter {
    /// 只有单个规范 UUID 能被选中，其他输入保留为完整的非法原文。
    pub(crate) fn from_values(values: Vec<String>) -> Self {
        match values.as_slice() {
            [] => Self::Unspecified,
            [value] if valid_uuid(value) => Self::Selected(value.clone()),
            _ => Self::Invalid(values),
        }
    }

    /// 只有经校验的场次可以作为请求目标。
    pub(crate) fn selected_id(&self) -> Option<&str> {
        match self {
            Self::Selected(id) => Some(id),
            Self::Unspecified | Self::Invalid(_) => None,
        }
    }

    /// 写回地址时保留非法参数原有的值和重复次数。
    pub(crate) fn query_values(&self) -> &[String] {
        match self {
            Self::Unspecified => &[],
            Self::Selected(id) => std::slice::from_ref(id),
            Self::Invalid(values) => values,
        }
    }
}

/// 公共查询的场次状态。结构体更新语法需要访问该字段，内部状态由查询层写入。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StoredAuction(AuctionParameter);

impl PublicBoardQuery {
    /// 读取已解析的场次，非法原文不作为场次 ID 投影。
    pub(crate) fn auction_parameter(&self) -> &AuctionParameter {
        &self.auction.0
    }

    /// 由场次状态派生合法 ID，供只需要读取目标的调用方使用。
    pub(crate) fn auction_id(&self) -> Option<&str> {
        self.auction_parameter().selected_id()
    }

    /// 用与地址解析相同的规则构造场次，给链接和查询替身共用。
    pub(crate) fn with_auction_values(mut self, values: impl IntoIterator<Item = String>) -> Self {
        self.set_auction_parameter(AuctionParameter::from_values(values.into_iter().collect()));
        self
    }

    pub(super) fn set_auction_parameter(&mut self, auction: AuctionParameter) {
        self.auction = StoredAuction(auction);
    }
}

/// 完整替换场次。`None` 明确清除场次，自动纠偏必须保留非法的书签。
pub(crate) enum AuctionSelection {
    Explicit(Option<String>),
    Automatic(Option<String>),
}

impl AuctionSelection {
    pub(super) fn mode(&self) -> QueryWriteMode {
        match self {
            Self::Explicit(_) => QueryWriteMode::Push,
            Self::Automatic(_) => QueryWriteMode::Replace,
        }
    }
}
