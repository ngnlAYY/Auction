//! Admin 拍卖 lot 页面装配。

mod edit;
mod page;
mod presentation;
mod proxy;
mod row;
mod state;
mod table;

pub(super) use page::page;
pub(super) use presentation::{MAX_START_PRICE, admin_lot_code_for, parse_start_price};
