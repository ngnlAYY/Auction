//! Public 只读查询处理器的模块装配。
//! 世界/拍卖目录、竞价与价格历史、lot 列表/详情分别由子模块拥有；本模块只保留稳定路由
//! 出口，具体应用端口和 DTO 边界继续由父级 `routes` 模块提供。

use super::*;

mod auctions;
mod history;
mod lots;
mod world;

pub(crate) use auctions::{current_auction, list_auctions, list_auctions_v2};
pub(crate) use history::{list_bid_log, list_price_history};
pub(crate) use lots::{list_lots, lot_detail};
pub(crate) use world::list_worlds;
