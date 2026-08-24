//! Admin Users owner 的用户管理端口装配。
//!
//! 安全投影、命令/结果和窄端口分别由子模块拥有；这里保留原有
//! `auction_application::ports::*` 公共导出路径。

mod commands;
mod common;
mod model;
mod ports;

pub use commands::*;
pub use model::*;
pub use ports::*;
