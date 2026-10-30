//! Public 只读历史页面的职责分组。

use auction_web_shared::legacy_t as t;
use dioxus::prelude::*;

use crate::api::{
    BidLogEntry, BidLogPage, PriceHistoryEntry, PriceHistoryFetchError, PriceHistoryPage,
    PriceHistoryQuery, PriceHistoryStats, fetch_bid_log_auctions, fetch_bid_log_page,
    fetch_price_history, fetch_worlds, lot_code_for,
};

use super::{
    AsyncState, AsyncStateKind, BidHistoryParticipant, PublicBoardQuery, PublicPager,
    QueryWriteMode, Time, catalog_category_key, catalog_category_label, current_public_pathname,
    format_amount, public_page_matches, supported_worlds, update_public_page_size,
    update_public_query_state, use_public_query_state, world_label, write_public_query,
};

mod bid_log;
mod history;

pub(crate) use bid_log::bid_log;
pub(crate) use history::history;
