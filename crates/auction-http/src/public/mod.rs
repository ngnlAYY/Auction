//! Public HTTP 适配层：handler、路由装配与共享辅助转出。
//! handler 只做 HTTP 输入到应用命令、应用输出到稳定契约的映射；`account`、`competition`、
//! `session`、`session_preference`、`watch`、`watch_preference` 拥有 Public 路由装配；
//! 本模块把组内共同依赖的辅助显式转出，使子模块可以继续以 `super::` 相对引用。

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

pub(crate) use crate::mapping::{
    api_error, not_found_error, parse_positive_i64, parse_public_uuid, parse_query,
    public_timestamp,
};
pub(crate) use crate::server::build_identity;
pub(crate) use crate::session::{
    authenticated_public_identity, is_unauthenticated, read_public_session_cookie,
    realtime_connection_guard, session_unavailable,
};
pub use crate::state::PublicApiState;
