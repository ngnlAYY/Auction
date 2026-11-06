//! Public HTTP handler：公开/匿名读取与竞价命令的适配层。
//! 路由装配与共享响应辅助由同级 `routes` 拥有，本模块把 Public handler 共同依赖的辅助
//! 显式转出，使组内模块可以继续以 `super::` 相对引用错误、会话与实时辅助。

pub(crate) mod blacklist;
pub(crate) mod compensation;
pub(crate) mod countdown;
pub(crate) mod forum;
pub(crate) mod realtime;
pub(crate) mod registration;
pub(crate) mod rules;

pub(crate) use super::routes::{
    PublicApiState, api_error, authenticated_public_identity, build_identity, is_unauthenticated,
    not_found_error, parse_positive_i64, parse_public_uuid, parse_query, public_timestamp,
    read_public_session_cookie, realtime_connection_guard, realtime_hub, session_unavailable,
};
