//! Public 拍卖、lot、竞价日志和价格历史查询。
//! `cached_get` 拥有条件 GET、非 2xx 响应的缓存分类与带缓存的严格解码，其余子模块按读取对象
//! 划分：`current`（世界列表、当前场次与按 ID 打开的历史场次）、`directory`（场次目录分页）、
//! `bid_log`（竞价日志与按状态的场次目录遍历）、`lots`（lot 目录与详情）、`price_history`（价格历史）。

mod bid_log;
mod cached_get;
mod current;
mod directory;
mod lots;
mod price_history;

pub(super) use bid_log::fetch_bid_log_auction_status;
pub(crate) use bid_log::{fetch_bid_log_auctions, fetch_bid_log_page, fetch_lot_bid_log};
pub(super) use cached_get::cached_public_read;
pub(crate) use current::{
    fetch_current_auction, fetch_historical_auction, fetch_worlds, revalidate_worlds,
};
pub(crate) use directory::{fetch_auction_history_page, fetch_auction_list};
pub(crate) use lots::{
    LotDetailFetchError, fetch_current_lots, fetch_lot_detail, prefetch_current_lots,
};
pub(crate) use price_history::{PriceHistoryFetchError, fetch_price_history};
