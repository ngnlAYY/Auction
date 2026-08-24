//! Admin runtime 配置命令账本适配器的模块装配。
//!
//! runtime 文件和数据库账本是两个独立的持久化资源；本目录按跨后端分派、PostgreSQL
//! 查询、事务 mutation、行映射和错误分类拆分，实际账本仍只由本适配器拥有。

#[path = "runtime/dispatch.rs"]
mod dispatch;
#[path = "runtime/errors.rs"]
mod errors;
#[path = "runtime/mapping.rs"]
mod mapping;
#[path = "runtime/mutations.rs"]
mod mutations;
#[path = "runtime/queries.rs"]
mod queries;

pub use dispatch::DatabaseAdminRuntimeCommands;
