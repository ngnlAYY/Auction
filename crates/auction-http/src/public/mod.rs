//! Public HTTP 适配层；路由表在 `server/surfaces.rs`。
//! handler 只做 HTTP 输入到应用命令、应用输出到稳定契约的映射。本模块只为路由装配转出私有
//! 子模块的 handler，并转出 `crate::session` 读取的 Public 会话 Cookie 名；会话辅助、状态、
//! 构建身份、错误外壳、查询解析与时间戳由子模块从 `session`、`state`、`server`、`errors`、
//! `query`、`mapping`、`middleware` 的定义处直接导入。

mod account;
mod competition;
mod session;
mod session_preference;
mod watch;
mod watch_preference;

pub(crate) mod blacklist;
pub(crate) mod browser_errors;
pub(crate) mod compensation;
pub(crate) mod countdown;
pub(crate) mod forum;
pub(crate) mod realtime;
pub(crate) mod registration;
pub(crate) mod rules;

pub(crate) use account::{get_my_proxy_authorization, list_my_bids};
pub(crate) use competition::{create_proxy_authorization, lower_proxy_authorization, place_bid};
pub(crate) use session::{
    PUBLIC_SESSION_COOKIE, current_session, login_session, logout_session, refresh_session_cookie,
};
pub(crate) use session_preference::{get_session_preference, set_session_preference};
pub(crate) use watch::{list_lot_watches, set_lot_watch};
pub(crate) use watch_preference::{get_watch_preference, set_watch_preference};
