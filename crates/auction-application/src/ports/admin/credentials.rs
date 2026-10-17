//! Admin 凭据管理端口。
//! 凭据属于完整 PostgreSQL 档：Admin 只能看到不含明文和密文的安全投影，明文只在导入
//! 命令或外部只读校验的集成适配器中短暂存在。值对象、命令载荷和端口接口分别由子模块拥有，本文件
//! 只负责稳定装配与公共导出。

#[path = "credentials/check.rs"]
mod check;
#[path = "credentials/commands.rs"]
mod commands;
#[path = "credentials/identity.rs"]
mod identity;
#[path = "credentials/ports.rs"]
mod ports;
#[path = "credentials/value/mod.rs"]
mod value;

pub use check::*;
pub use commands::*;
pub use identity::*;
pub use ports::*;
pub use value::*;
