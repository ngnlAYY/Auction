//! Public 只读历史页面的职责分组。

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api::{
    BidLogEntry, BidLogPage, PriceHistoryEntry, PriceHistoryPage, PriceHistoryQuery,
    PriceHistoryStats, fetch_bid_log_auctions, fetch_bid_log_page, fetch_price_history,
    fetch_worlds, lot_code_for,
};

use super::pagination::PublicPager;
use super::{
    CatalogParticipant, Placeholder, PublicBoardQuery, RetryPlaceholder, Route,
    catalog_category_key, catalog_category_label, enabled_worlds, format_amount,
    format_auction_time, read_public_query, update_public_query, world_label,
};

mod bid_log;
#[path = "history/mod.rs"]
mod history;

pub(crate) use bid_log::bid_log;
pub(crate) use history::history;
