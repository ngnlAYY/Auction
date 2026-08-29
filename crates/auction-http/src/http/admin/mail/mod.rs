//! Admin 邮件 HTTP 适配器的模块装配。
//!
//! 轮询状态、入站安全投影、状态命令和 Worker 唤醒分别由子模块拥有；共享错误映射与
//! HTTP 边界集中在 `common.rs`，本模块只保留稳定处理器出口。

#[path = "commands.rs"]
mod commands;
#[path = "common.rs"]
mod common;
#[path = "intake.rs"]
mod intake;
#[path = "state.rs"]
mod state;
#[path = "trigger.rs"]
mod trigger;

pub(super) use super::{
    AdminApiState, admin_session_guard, api_error, authenticated_admin_mutation,
    authenticated_admin_session, internal_error, no_store_headers, not_found_error,
    parse_positive_i64, parse_query, request_id, timestamp,
};
pub(super) use commands::set_paused;
pub(super) use common::{feature_unavailable, map_error};
pub(super) use intake::{detail_intake, list_intakes};
pub(super) use state::state;
pub(super) use trigger::trigger_poll;
