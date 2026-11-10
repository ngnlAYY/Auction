//! Admin HTTP handler：按领域分组的严格 Admin 适配层。
//! 这里只做 HTTP 输入到应用命令、应用输出到稳定契约的映射；路由装配与共享响应辅助由
//! 同级 `routes` 拥有，本模块把 Admin handler 共同依赖的辅助显式转出，使组内模块可以
//! 继续以 `super::` 相对引用错误、会话与映射辅助。

pub(crate) mod auctions;
pub(crate) mod audit;
pub(crate) mod credentials;
pub(crate) mod dashboard;
pub(crate) mod delivery;
pub(crate) mod forum;
pub(crate) mod intake;
pub(crate) mod lot_update;
pub(crate) mod lots;
pub(crate) mod mail;
pub(crate) mod proxy_events;
pub(crate) mod realtime;
pub(crate) mod renames;
pub(crate) mod rules;
pub(crate) mod runtime_config;
pub(crate) mod settlement_catalog;
pub(crate) mod users;
pub(crate) mod worlds;

pub(crate) use super::routes::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_mutation,
    authenticated_admin_session, feature_unavailable, internal_error, invalid_query,
    no_store_headers, not_found_error, parse_canonical_uuid, parse_positive_i64, parse_query,
    request_id, storage_unavailable, strict_json, timestamp,
};
