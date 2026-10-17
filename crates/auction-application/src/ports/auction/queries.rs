//! 公开只读查询端口的稳定公共出口。

#[path = "queries/account.rs"]
mod account;
#[path = "queries/auction.rs"]
mod auction;
#[path = "queries/bid_log.rs"]
mod bid_log;
#[path = "queries/countdown.rs"]
mod countdown;
#[path = "queries/lot.rs"]
mod lot;
#[path = "queries/price_history.rs"]
mod price_history;
#[path = "queries/world.rs"]
mod world;

pub use account::*;
pub use auction::*;
pub use bid_log::*;
pub use countdown::*;
pub use lot::*;
pub use price_history::*;
pub use world::*;
