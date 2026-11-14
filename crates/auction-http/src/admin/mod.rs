//! Admin HTTP 适配层：handler 与共享辅助转出；路由表在 `server/surfaces.rs`。
//! handler 只做 HTTP 输入到应用命令、应用输出到稳定契约的映射；本模块转出私有子模块的
//! handler，并显式转出组内共同依赖的错误、会话与映射辅助，供子模块以 `super::` 引用。

mod lifecycle;
mod recovery;
mod reports;
mod session;

pub(crate) mod alerts;
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
pub(crate) mod mail_templates;
pub(crate) mod proxy_events;
pub(crate) mod realtime;
pub(crate) mod renames;
pub(crate) mod rules;
pub(crate) mod runtime_config;
pub(crate) mod runtime_switches;
pub(crate) mod settlement_catalog;
pub(crate) mod users;
pub(crate) mod worlds;

pub(crate) const ADMIN_SESSION_COOKIE: &str = "auction_admin_session";
pub(crate) const ADMIN_CSRF_COOKIE: &str = "auction_admin_csrf";
pub(crate) const ADMIN_CSRF_HEADER: &str = "x-csrf-token";

pub(crate) use lifecycle::{
    admin_cancel_auction, admin_early_settlement_auction, admin_final_phase_auction,
    admin_open_auction, admin_preview_auction,
};
pub(crate) use recovery::admin_confirm_recovery;
pub(crate) use reports::{admin_recovery_report, admin_settlement_report};
pub(crate) use session::{current_admin_session, login_admin_session, logout_admin_session};

pub(crate) use crate::mapping::{
    api_error, feature_unavailable, invalid_query, not_found_error, parse_canonical_uuid,
    parse_positive_i64, parse_query, request_id, storage_unavailable, timestamp,
};
pub(crate) use crate::middleware::strict_json;
pub(crate) use crate::session::{
    admin_session_guard, authenticated_admin_mutation, authenticated_admin_session, internal_error,
    no_store_headers,
};
pub use crate::state::AdminApiState;
