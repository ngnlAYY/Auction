//! Public 页面组件装配。

mod history;
mod lot;
mod routes;

// 父 surface 统一把这些出口 re-export 给 Dioxus 路由与目录页。
pub(crate) use history::AuctionList;
pub(crate) use lot::{BidHistoryTable, LotDetail};
pub(crate) use routes::{
    Account, BidLog, Blacklist, Compensation, CompensationPool, Forum, History, Login, Me,
    NotFound, Rules, Status,
};
