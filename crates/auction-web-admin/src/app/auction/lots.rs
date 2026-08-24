//! Admin 拍卖 lot 页面装配。

#[path = "lots/helpers.rs"]
mod helpers;
#[path = "lots/page.rs"]
mod page;
#[path = "lots/proxy.rs"]
mod proxy;
#[path = "lots/row.rs"]
mod row;
#[path = "lots/state.rs"]
mod state;
#[path = "lots/table.rs"]
mod table;

pub(super) use page::page;
