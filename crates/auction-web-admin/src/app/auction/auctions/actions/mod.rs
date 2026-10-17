//! Admin 拍卖生命周期操作的稳定出口。
//! 行渲染、确认对话框和动作文案/可用性模型分别归属相邻 owner；本文件只负责模块装配与
//! 对外组件出口。

mod dialog;
mod model;
mod row;
mod schedule;

pub(super) use model::{AdminAuctionNotice, auction_notice_label};
pub(super) use row::AdminAuctionRow;
