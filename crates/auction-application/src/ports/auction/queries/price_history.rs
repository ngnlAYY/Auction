//! Public 成交价格历史查询端口。

use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::core::ApplicationFuture;

/// 公开价格历史允许的最大名称词条数。
pub const MAX_PUBLIC_HISTORY_NAME_TERMS: usize = 8;

/// 公开价格历史兼容页允许的最大 offset。
pub const MAX_PUBLIC_PRICE_HISTORY_OFFSET: i64 = 10_000;
/// Public 价格历史单页的最大条数。
pub const MAX_PUBLIC_PRICE_HISTORY_LIMIT: i64 = 200;

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
    pub closed_from: Option<Timestamp>,
    pub closed_before: Option<Timestamp>,
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
            || self.limit > MAX_PUBLIC_PRICE_HISTORY_LIMIT
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
    Boundary { closed_at: Timestamp, auction_sequence: i64, lot_number: i64, lot_id: Uuid },
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
    pub closed_at: Timestamp,
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
        if self.count == 0 && self.median_exact.is_some() {
            return Err("空统计不能包含精确中位数");
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

impl PublicPriceHistoryPage {
    /// 校验适配器返回的条目、排序和分页状态均与原请求一致。
    pub fn validate(&self, request: &PublicPriceHistoryListRequest) -> Result<(), &'static str> {
        request.validate()?;
        let entry_count =
            i64::try_from(self.entries.len()).map_err(|_| "公开价格历史页大小溢出")?;
        if self.limit != request.limit || self.offset != request.offset || entry_count > self.limit
        {
            return Err("公开价格历史页分页参数无效");
        }
        let mut seen_lot_ids = std::collections::HashSet::with_capacity(self.entries.len());
        if self
            .entries
            .iter()
            .any(|entry| !valid_price_history_entry(entry) || !seen_lot_ids.insert(entry.lot_id))
            || self
                .entries
                .windows(2)
                .any(|pair| history_entry_order(&pair[0], &pair[1], request.sort).is_gt())
        {
            return Err("公开价格历史页包含无效条目或排序");
        }

        if let Some(cursor) = request.cursor.as_ref() {
            if self.offset != 0
                || self.total.is_some()
                || self.stats.is_some()
                || self.has_more != self.next_cursor.is_some()
                || self.has_more && entry_count != self.limit
                || self.entries.iter().any(|entry| !after_history_cursor(entry, cursor))
            {
                return Err("公开价格历史游标页状态无效");
            }
            let expected_next =
                self.has_more.then(|| self.entries.last().map(history_entry_cursor)).flatten();
            if self.next_cursor != expected_next {
                return Err("公开价格历史下一游标无效");
            }
        } else {
            let (Some(total), Some(stats)) = (self.total, self.stats.as_ref()) else {
                return Err("公开价格历史编号页缺少统计");
            };
            let expected_count = total.saturating_sub(self.offset).clamp(0, self.limit);
            if total < 0
                || stats.validate().is_err()
                || stats.count != total
                || entry_count != expected_count
                || self.next_cursor.is_some()
                || self.has_more
            {
                return Err("公开价格历史编号页状态无效");
            }
        }
        Ok(())
    }
}

fn valid_price_history_entry(entry: &PublicPriceHistoryEntry) -> bool {
    !entry.lot_id.is_nil()
        && !entry.auction_id.is_nil()
        && matches!(entry.world_id.as_str(), "main" | "isekai")
        && auction_domain::valid_text(&entry.world_id, 64)
        && entry.auction_sequence > 0
        && entry.ordering_lot_number > 0
        && entry.lot_number > 0
        && auction_domain::valid_text(&entry.equipment_name, 512)
        && auction_domain::valid_text(&entry.equipment_category, 128)
        && entry.equipment_level.is_none_or(|level| level >= 0)
        && auction_domain::valid_text(&entry.buyer_name, 256)
        && auction_domain::valid_text(&entry.seller_name, 256)
        && entry.price > 0
        && entry.closed_at.timestamp_micros() != 0
}

fn history_entry_cursor(entry: &PublicPriceHistoryEntry) -> PublicPriceHistoryCursor {
    PublicPriceHistoryCursor::Boundary {
        closed_at: entry.closed_at,
        auction_sequence: entry.auction_sequence,
        lot_number: entry.ordering_lot_number,
        lot_id: entry.lot_id,
    }
}

fn after_history_cursor(
    entry: &PublicPriceHistoryEntry,
    cursor: &PublicPriceHistoryCursor,
) -> bool {
    match cursor {
        PublicPriceHistoryCursor::Start => true,
        PublicPriceHistoryCursor::Boundary { closed_at, auction_sequence, lot_number, lot_id } => {
            entry.closed_at < *closed_at
                || entry.closed_at == *closed_at
                    && (entry.auction_sequence < *auction_sequence
                        || entry.auction_sequence == *auction_sequence
                            && (entry.ordering_lot_number > *lot_number
                                || entry.ordering_lot_number == *lot_number
                                    && entry.lot_id > *lot_id))
        }
    }
}

fn history_entry_order(
    left: &PublicPriceHistoryEntry,
    right: &PublicPriceHistoryEntry,
    sort: PublicPriceHistorySort,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    if matches!(sort, PublicPriceHistorySort::PriceAsc | PublicPriceHistorySort::PriceDesc) {
        let price_order = if sort == PublicPriceHistorySort::PriceAsc {
            left.price.cmp(&right.price)
        } else {
            right.price.cmp(&left.price)
        };
        if price_order != Ordering::Equal {
            return price_order;
        }
    }
    let closed_order = if sort == PublicPriceHistorySort::ClosedAsc {
        left.closed_at.cmp(&right.closed_at)
    } else {
        right.closed_at.cmp(&left.closed_at)
    };
    closed_order
        .then_with(|| right.auction_sequence.cmp(&left.auction_sequence))
        .then_with(|| left.ordering_lot_number.cmp(&right.ordering_lot_number))
        .then_with(|| left.lot_id.cmp(&right.lot_id))
}

/// Public 价格历史只读端口。
pub trait PublicPriceHistoryReader: Send + Sync {
    fn list_public_price_history(
        &self,
        request: PublicPriceHistoryListRequest,
    ) -> ApplicationFuture<'_, PublicPriceHistoryPage>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minute: u32) -> Timestamp {
        Timestamp::from_ymd_hms(2026, 11, 20, 12, minute, 0)
            .unwrap_or_else(|| panic!("固定测试时间必须存在"))
    }

    fn request(
        cursor: Option<PublicPriceHistoryCursor>,
        sort: PublicPriceHistorySort,
        limit: i64,
        offset: i64,
    ) -> PublicPriceHistoryListRequest {
        PublicPriceHistoryListRequest {
            name_terms: Vec::new(),
            world_id: None,
            closed_from: None,
            closed_before: None,
            minimum_price: None,
            maximum_price: None,
            buyer: None,
            seller: None,
            sort,
            limit,
            offset,
            cursor,
        }
    }

    fn entry(id: u128, minute: u32, price: i64) -> PublicPriceHistoryEntry {
        PublicPriceHistoryEntry {
            lot_id: Uuid::from_u128(id),
            auction_id: Uuid::from_u128(100),
            world_id: "main".to_owned(),
            auction_sequence: 1,
            ordering_lot_number: id as i64,
            lot_number: id as i64,
            equipment_name: format!("Equipment {id}"),
            equipment_category: "Staff".to_owned(),
            equipment_level: None,
            buyer_name: "buyer".to_owned(),
            seller_name: "seller".to_owned(),
            price,
            closed_at: at(minute),
        }
    }

    fn single_stats() -> PublicPriceHistoryStats {
        PublicPriceHistoryStats {
            count: 1,
            minimum: 1_000,
            maximum: 1_000,
            median: 1_000,
            median_exact: Some("1000".to_owned()),
        }
    }

    #[test]
    fn numbered_page_checks_snapshot_counts_and_sort_order() {
        let request = request(None, PublicPriceHistorySort::ClosedDesc, 2, 0);
        let page = PublicPriceHistoryPage {
            entries: vec![entry(1, 10, 1_000)],
            total: Some(1),
            stats: Some(single_stats()),
            limit: 2,
            offset: 0,
            next_cursor: None,
            has_more: false,
        };
        assert_eq!(page.validate(&request), Ok(()));

        let mut duplicate = page.clone();
        duplicate.entries = vec![entry(1, 10, 1_000), entry(1, 10, 1_000)];
        duplicate.total = Some(2);
        duplicate.stats = Some(PublicPriceHistoryStats {
            count: 2,
            minimum: 1_000,
            maximum: 1_000,
            median: 1_000,
            median_exact: Some("1000".to_owned()),
        });
        assert!(duplicate.validate(&request).is_err());
        let mut inconsistent = page;
        inconsistent.entries = vec![entry(1, 9, 1_000), entry(2, 10, 1_000)];
        inconsistent.total = Some(2);
        inconsistent.stats = Some(PublicPriceHistoryStats {
            count: 2,
            minimum: 1_000,
            maximum: 1_000,
            median: 1_000,
            median_exact: Some("1000".to_owned()),
        });
        assert!(inconsistent.validate(&request).is_err());
    }

    #[test]
    fn numbered_page_accepts_empty_rows_past_the_last_page() {
        let request = request(None, PublicPriceHistorySort::ClosedDesc, 2, 4);
        let page = PublicPriceHistoryPage {
            entries: Vec::new(),
            total: Some(1),
            stats: Some(single_stats()),
            limit: request.limit,
            offset: request.offset,
            next_cursor: None,
            has_more: false,
        };
        assert_eq!(page.validate(&request), Ok(()));
    }

    #[test]
    fn numbered_page_rejects_short_rows_and_mismatched_statistics() {
        let request = request(None, PublicPriceHistorySort::ClosedDesc, 2, 0);
        let page = PublicPriceHistoryPage {
            entries: vec![entry(1, 10, 1_000)],
            total: Some(2),
            stats: Some(PublicPriceHistoryStats { count: 1, ..single_stats() }),
            limit: 2,
            offset: 0,
            next_cursor: None,
            has_more: false,
        };
        assert!(page.validate(&request).is_err());
    }

    #[test]
    fn cursor_page_checks_boundary_and_next_cursor_consistency() {
        let boundary = PublicPriceHistoryCursor::Boundary {
            closed_at: at(10),
            auction_sequence: 1,
            lot_number: 1,
            lot_id: Uuid::from_u128(1),
        };
        let request = request(Some(boundary), PublicPriceHistorySort::ClosedDesc, 2, 0);
        let first = entry(2, 9, 1_000);
        let terminal_page = PublicPriceHistoryPage {
            entries: vec![first.clone()],
            total: None,
            stats: None,
            limit: 2,
            offset: 0,
            next_cursor: None,
            has_more: false,
        };
        assert_eq!(terminal_page.validate(&request), Ok(()));
        let second = entry(3, 8, 1_000);
        let next_cursor = history_entry_cursor(&second);
        let page = PublicPriceHistoryPage {
            entries: vec![first, second],
            total: None,
            stats: None,
            limit: 2,
            offset: 0,
            next_cursor: Some(next_cursor),
            has_more: true,
        };
        assert_eq!(page.validate(&request), Ok(()));

        let mut missing_cursor = page.clone();
        missing_cursor.next_cursor = None;
        assert!(missing_cursor.validate(&request).is_err());

        let mut mismatched_cursor = page.clone();
        mismatched_cursor.next_cursor = Some(PublicPriceHistoryCursor::Start);
        assert!(mismatched_cursor.validate(&request).is_err());

        let mut before_boundary = page;
        before_boundary.entries[0] = entry(2, 11, 1_000);
        assert!(before_boundary.validate(&request).is_err());
    }
}
