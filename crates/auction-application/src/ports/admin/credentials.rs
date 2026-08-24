//! Admin 凭据管理端口。
//!
//! 凭据属于完整 PostgreSQL 档：Admin 只能看到不含明文和密文的安全投影，明文只在导入
//! 命令和加密适配器之间短暂流动。值对象、命令载荷和端口接口分别由子模块拥有，本文件
//! 只负责稳定装配与公共导出。

#[path = "credentials/commands.rs"]
mod commands;
#[path = "credentials/ports.rs"]
mod ports;
#[path = "credentials/value/mod.rs"]
mod value;

pub use commands::*;
pub use ports::*;
pub use value::*;
