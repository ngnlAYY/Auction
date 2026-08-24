//! `auctionctl` PostgreSQL 命令的稳定装配出口。
//!
//! 凭据引导、只读预检、备份归档和专用恢复/重建 owner 的命令桥接分别位于子模块；CLI 名称、
//! 确认门槛、退出码和私有回执路径由上层分派继续统一维护。

mod backup;
mod bootstrap;
mod delegation;
mod preflight;

#[cfg(feature = "postgres")]
pub(crate) use backup::*;
pub(crate) use bootstrap::*;
pub(crate) use delegation::*;
pub(crate) use preflight::*;
