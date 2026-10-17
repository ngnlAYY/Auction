//! Admin 会话、生命周期、结算与恢复 HTTP 处理器的模块装配。
//! 各组处理器保留在同一 Admin surface 的稳定导出下；具体会话、生命周期、报告和恢复确认
//! 逻辑分别由子模块拥有。

#[path = "admin/lifecycle.rs"]
mod lifecycle;
#[path = "admin/recovery.rs"]
mod recovery;
#[path = "admin/reports.rs"]
mod reports;
#[path = "admin/session.rs"]
mod session;

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
