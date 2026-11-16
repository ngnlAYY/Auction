//! Admin HTTP 适配层；路由表在 `server/surfaces.rs`。
//! handler 只做 HTTP 输入到应用命令、应用输出到稳定契约的映射。本模块只为路由装配转出私有
//! 子模块的 handler，并定义 `crate::session` 与本组 `session` 子模块共用的 Admin 会话 Cookie、
//! CSRF Cookie 与 CSRF 请求头名；会话守卫、状态、错误外壳、查询解析与时间戳由子模块从
//! `session`、`state`、`errors`、`query`、`mapping`、`middleware` 的定义处直接导入。

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
pub(crate) mod settlement_returns;
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
