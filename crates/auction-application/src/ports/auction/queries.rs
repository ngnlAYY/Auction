//! 公开只读查询端口的稳定公共出口。
//!
//! 各查询 owner 保持独立实现，避免目录中的拍卖、lot、账户和历史投影互相堆叠；
//! 这里仅负责模块装配和兼容既有的 `crate::` 公共路径。

#[path = "queries/account.rs"]
mod account;
#[path = "queries/auction.rs"]
mod auction;
#[path = "queries/bid_log.rs"]
mod bid_log;
#[path = "queries/lot.rs"]
mod lot;
#[path = "queries/price_history.rs"]
mod price_history;
#[path = "queries/world.rs"]
mod world;

pub use account::*;
pub use auction::*;
pub use bid_log::*;
pub use lot::*;
pub use price_history::*;
pub use world::*;
