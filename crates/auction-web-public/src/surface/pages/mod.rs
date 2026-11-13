//! Public 页面组件装配。

mod history;
mod routes;

// 父 surface 统一把这些出口 re-export 给 Dioxus 路由与目录页。
pub(crate) use history::AuctionList;
pub(crate) use routes::{
    About, Account, BidLog, Blacklist, Compensation, CompensationPool, Forum, History, Login,
    LotDetail, Me, NotFound, Rules, Status,
};
