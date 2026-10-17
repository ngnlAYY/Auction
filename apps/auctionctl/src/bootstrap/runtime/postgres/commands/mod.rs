//! `auctionctl` PostgreSQL 命令的稳定装配出口；CLI 名称、确认门槛、
//! 退出码和私有回执路径由上层分派统一维护。

mod backup;
mod bootstrap;
mod database_bootstrap;
mod delegation;
mod draft_soft_close;
pub(crate) use draft_soft_close::*;
mod forum;
mod lot_watch_pm;
pub(crate) use lot_watch_pm::*;
mod preflight;
mod rules;
mod settlement;

#[cfg(feature = "postgres")]
pub(crate) use backup::*;
pub(crate) use bootstrap::*;
pub(crate) use database_bootstrap::*;
pub(crate) use delegation::*;
pub(crate) use forum::*;
pub(crate) use preflight::*;
pub(crate) use rules::*;
pub(crate) use settlement::*;
