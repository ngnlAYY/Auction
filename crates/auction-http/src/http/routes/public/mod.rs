//! Public HTTP handler 的模块装配。
//! 会话、账户读取和竞价命令分别由子模块持有；本文件只负责稳定导出，路由路径与契约
//! 仍由 `routes/mod.rs` 统一装配。

// 子处理器沿用路由层的契约、状态和边界辅助函数；先在本模块引入，再由子模块按需使用。
use super::*;

mod account;
mod competition;
mod session;
mod session_preference;
mod watch;
mod watch_preference;
pub(crate) use watch_preference::{get_watch_preference, set_watch_preference};

pub(crate) use watch::{list_lot_watches, set_lot_watch};

pub(crate) use account::{get_my_proxy_authorization, list_my_bids};
pub(crate) use competition::{create_proxy_authorization, lower_proxy_authorization, place_bid};
pub(crate) use session::{
    PUBLIC_SESSION_COOKIE, current_session, login_session, logout_session, refresh_session_cookie,
};
pub(crate) use session_preference::{get_session_preference, set_session_preference};
