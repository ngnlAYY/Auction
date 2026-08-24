//! Public 成交价格历史查询端口。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ApplicationFuture;

/// 公开价格历史允许的最大名称词条数。
pub const MAX_PUBLIC_HISTORY_NAME_TERMS: usize = 8;

/// 公开价格历史允许的最大页大小。
pub const MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE: i64 = 200;

/// 公开价格历史兼容页允许的最大 offset。
pub const MAX_PUBLIC_PRICE_HISTORY_OFFSET: i64 = 10_000;

/// 公开价格历史的筛选与分页条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPriceHistoryListRequest {
    pub name_terms: Vec<String>,
    pub world_id: Option<String>,
    pub closed_from: Option<DateTime<Utc>>,
    pub closed_before: Option<DateTime<Utc>>,
    pub minimum_price: Option<i64>,
    pub maximum_price: Option<i64>,
    pub buyer: Option<String>,
    pub seller: Option<String>,
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
            || self.limit > MAX_PUBLIC_PRICE_HISTORY_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_PRICE_HISTORY_OFFSET
            || self.cursor.is_some() && self.offset != 0
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicPriceHistoryStats {
    pub count: i64,
    pub minimum: i64,
    pub maximum: i64,
    pub median: i64,
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
