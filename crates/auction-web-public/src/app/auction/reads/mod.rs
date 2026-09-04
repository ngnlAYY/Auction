//! Public 只读历史页面的职责分组。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{
    BidLogEntry, BidLogPage, PriceHistoryEntry, PriceHistoryFetchError, PriceHistoryPage,
    PriceHistoryQuery, PriceHistoryStats, fetch_bid_log_auctions, fetch_bid_log_page,
    fetch_price_history, fetch_worlds, lot_code_for,
};

use super::pagination::PublicPager;
use super::{
    BidHistoryParticipant, Placeholder, PublicBoardQuery, RetryPlaceholder, Route,
    catalog_category_key, catalog_category_label, format_amount, format_auction_time,
    read_public_query, set_public_query, supported_worlds, update_public_query, world_label,
};

mod bid_log;
#[path = "history/mod.rs"]
mod history;

pub(crate) use bid_log::bid_log;
pub(crate) use history::history;
