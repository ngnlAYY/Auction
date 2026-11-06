//! Public 成交价格历史查询端口。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ports::auction::ApplicationFuture;

/// 公开价格历史允许的最大名称词条数。
pub const MAX_PUBLIC_HISTORY_NAME_TERMS: usize = 8;

/// 公开价格历史兼容页允许的最大 offset。
pub const MAX_PUBLIC_PRICE_HISTORY_OFFSET: i64 = 10_000;

/// 成交历史编号页的服务端排序闭集；旧游标只对应默认的成交时间倒序。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PublicPriceHistorySort {
    #[default]
    ClosedDesc,
    ClosedAsc,
    PriceDesc,
    PriceAsc,
}

impl PublicPriceHistorySort {
    /// 返回 HTTP 与存储共用的固定排序名称，不接受自由 SQL 片段。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClosedDesc => "closed_desc",
            Self::ClosedAsc => "closed_asc",
            Self::PriceDesc => "price_desc",
            Self::PriceAsc => "price_asc",
        }
    }
}

/// 公开价格历史的筛选与分页条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPriceHistoryListRequest {
    /// 按空白或英文逗号拆分后的名称词条；全部按忽略大小写的字面包含条件匹配。
    pub name_terms: Vec<String>,
    pub world_id: Option<String>,
    pub closed_from: Option<DateTime<Utc>>,
    pub closed_before: Option<DateTime<Utc>>,
    pub minimum_price: Option<i64>,
    pub maximum_price: Option<i64>,
    pub buyer: Option<String>,
    pub seller: Option<String>,
    pub sort: PublicPriceHistorySort,
    pub limit: i64,
    pub offset: i64,
    pub cursor: Option<PublicPriceHistoryCursor>,
}

impl PublicPriceHistoryListRequest {
    /// 校验旧价格历史过滤器、半开时间区间和编号/cursor 分页边界。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name_terms.len() > MAX_PUBLIC_HISTORY_NAME_TERMS
            || self
                .name_terms
                .iter()
                .any(|term| term.is_empty() || term.len() > 128 || term != term.trim())
            || self
                .world_id
                .as_deref()
                .is_some_and(|world| world.is_empty() || world.len() > 64 || world != world.trim())
            || self.buyer.as_deref().is_some_and(|buyer| buyer.is_empty() || buyer.len() > 256)
            || self.seller.as_deref().is_some_and(|seller| seller.is_empty() || seller.len() > 256)
            || self.closed_from.is_some_and(|value| value.timestamp_micros() == 0)
            || self.closed_before.is_some_and(|value| value.timestamp_micros() == 0)
            || self.closed_from.zip(self.closed_before).is_some_and(|(from, before)| from >= before)
            || self.minimum_price.is_some_and(|price| price < 0)
            || self.maximum_price.is_some_and(|price| price < 0)
            || self
                .minimum_price
                .zip(self.maximum_price)
                .is_some_and(|(minimum, maximum)| minimum > maximum)
            || self.limit < 1
            || self.offset < 0
            || self.offset > MAX_PUBLIC_PRICE_HISTORY_OFFSET
            || self.cursor.is_some() && self.offset != 0
            || self.cursor.is_some() && self.sort != PublicPriceHistorySort::ClosedDesc
        {
            return Err("公开价格历史参数无效");
        }
        if self.name_terms.iter().any(|term| term.chars().any(char::is_control))
            || self.buyer.as_deref().is_some_and(|buyer| buyer.chars().any(char::is_control))
            || self.seller.as_deref().is_some_and(|seller| seller.chars().any(char::is_control))
        {
            return Err("公开价格历史文本参数无效");
        }
        if let Some(PublicPriceHistoryCursor::Boundary {
            closed_at,
            auction_sequence,
            lot_number,
            lot_id,
        }) = self.cursor.as_ref()
            && (closed_at.timestamp_micros() == 0
                || *auction_sequence <= 0
                || *lot_number <= 0
                || lot_id.is_nil())
        {
            return Err("公开价格历史游标无效");
        }
        Ok(())
    }
}

/// 公开价格历史的混合降序/升序 keyset 边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicPriceHistoryCursor {
    Start,
    Boundary { closed_at: DateTime<Utc>, auction_sequence: i64, lot_number: i64, lot_id: Uuid },
}

/// 已结算 sold lot 的公开成交事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPriceHistoryEntry {
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub world_id: String,
    pub auction_sequence: i64,
    /// 内部场次全局顺序，只用于稳定 keyset；对外编码使用 `lot_number` 分类编号。
    pub ordering_lot_number: i64,
    pub lot_number: i64,
    pub equipment_name: String,
    pub equipment_category: String,
    pub equipment_level: Option<i64>,
    pub buyer_name: String,
    pub seller_name: String,
    pub price: i64,
    pub closed_at: DateTime<Utc>,
}

/// 编号页返回的完整过滤集价格统计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPriceHistoryStats {
    pub count: i64,
    pub minimum: i64,
    pub maximum: i64,
    pub median: i64,
    /// 精确中位数的规范十进制字符串；旧适配器缺失此字段时仍保留整数兼容口径。
    pub median_exact: Option<String>,
}

impl PublicPriceHistoryStats {
    /// 校验完整统计及精确中位数；只接受整数或半个 Credit，不经浮点数转换。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.count < 0
            || self.count == 0 && (self.minimum != 0 || self.maximum != 0 || self.median != 0)
            || self.count > 0
                && (self.minimum <= 0
                    || self.maximum < self.minimum
                    || self.median < self.minimum
                    || self.median > self.maximum)
        {
            return Err("公开价格历史统计无效");
        }
        let Some(exact) = self.median_exact.as_deref() else { return Ok(()) };
        let (whole, half) = match exact.split_once('.') {
            Some((whole, "5")) => (whole, 1_i128),
            Some(_) => return Err("公开价格历史精确中位数词法无效"),
            None => (exact, 0),
        };
        if whole.is_empty()
            || whole.len() > 1 && whole.starts_with('0')
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("公开价格历史精确中位数词法无效");
        }
        let doubled = whole
            .parse::<i128>()
            .ok()
            .and_then(|value| value.checked_mul(2))
            .and_then(|value| value.checked_add(half))
            .ok_or("公开价格历史精确中位数溢出")?;
        // 旧整数 median 使用半数取偶：2.5 保留 2，3.5 取 4；精确字段始终保留 .5。
        let floor = doubled / 2;
        let legacy_median = floor + if half == 1 && floor % 2 == 1 { 1 } else { 0 };
        // 精确字符串仍限定在浏览器安全整数域；用二倍整数保留可能的 .5。
        if doubled > 9_007_199_254_740_991_i128 * 2
            || doubled < i128::from(self.minimum) * 2
            || doubled > i128::from(self.maximum) * 2
            || legacy_median != i128::from(self.median)
        {
            return Err("公开价格历史精确中位数范围无效");
        }
        Ok(())
    }
}

/// 公开价格历史页；cursor 页不计算全量统计。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPriceHistoryPage {
    pub entries: Vec<PublicPriceHistoryEntry>,
    pub total: Option<i64>,
    pub stats: Option<PublicPriceHistoryStats>,
    pub limit: i64,
    pub offset: i64,
    pub next_cursor: Option<PublicPriceHistoryCursor>,
    pub has_more: bool,
}

/// Public 价格历史只读端口。
pub trait PublicPriceHistoryReader: Send + Sync {
    fn list_public_price_history(
        &self,
        request: PublicPriceHistoryListRequest,
    ) -> ApplicationFuture<'_, PublicPriceHistoryPage>;
}
